//! Pattern files, `patterns/<name>.toml` (ADR 0028 §1–2, `project-format.md`
//! §12): note and step data as text. Read into the format's canonical
//! *source* (a header and a table of rows), which the library of the type
//! that names the pattern compiles into the instance's table when the plan
//! is built ([`signal_core::TableRule`]).
//!
//! Every table refuses unknown keys; every problem is a diagnostic at the
//! offending text.

use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::Deserialize;
use signal_core::Table;
use toml::Spanned;

use crate::valid_name;

/// The folder of pattern files in a project folder.
pub const PATTERNS: &str = "patterns";

/// The `kind` of a note pattern.
pub const NOTES: &str = "notes";
/// The `kind` of a step pattern.
pub const STEPS: &str = "steps";

/// The longest note pattern, in beats.
pub const MAX_LENGTH: f64 = 65_536.0;
/// The most notes in a pattern.
pub const MAX_NOTES: usize = 16_384;
/// The most steps in a step pattern.
pub const MAX_STEPS: i64 = 256;
/// The longest step, in beats.
pub const MAX_STEP: f64 = 64.0;
/// The most lanes in a step pattern.
pub const MAX_LANES: usize = 64;

/// A pattern file as read: its kind and its canonical source.
#[derive(Clone, Debug)]
pub(crate) struct Pattern {
    /// [`NOTES`] or [`STEPS`].
    pub(crate) kind: &'static str,
    /// `notes`: `[length]`; `steps`: `[steps, step]`.
    pub(crate) header: Vec<f64>,
    /// `notes`: `[key, start, length, velocity]` per note, sorted by
    /// (start, key); `steps`: `[level, c₀ … c₍steps−1₎]` per lane in file
    /// order, each cell 0 (rest), 1 (hit) or 2 (accent).
    pub(crate) rows: Table,
}

/// A note pattern file (`kind = "notes"`).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NotesFile {
    /// The format version of this file; this version of Signal reads 1.
    #[schemars(schema_with = "schema_version")]
    schema_version: Spanned<i64>,
    /// `"notes"`.
    #[schemars(schema_with = "notes_kind")]
    #[allow(dead_code)]
    kind: String,
    /// The loop length in beats, above 0 and at most 65536.
    #[schemars(with = "f64", range(min = 0.0, max = 65536.0))]
    length: Spanned<f64>,
    /// The notes, in any order (required; `notes = []` for none): an
    /// inline array of tables or `[[notes]]` tables.
    #[schemars(with = "Vec<NoteEntry>")]
    notes: Vec<Spanned<NoteEntry>>,
}

/// One note.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoteEntry {
    /// The key, an integer from 0 to 127 (60 = middle C by the library's
    /// convention).
    #[schemars(with = "i64", range(min = 0, max = 127))]
    key: Spanned<i64>,
    /// Where it starts, in beats from the pattern's start, at least 0.
    #[schemars(with = "f64", range(min = 0.0))]
    start: Spanned<f64>,
    /// How long it sounds, in beats, above 0.
    #[schemars(with = "f64", range(min = 0.0))]
    length: Spanned<f64>,
    /// Its velocity, from 0 to 1.
    #[schemars(with = "f64", range(min = 0.0, max = 1.0))]
    velocity: Spanned<f64>,
}

/// A step pattern file (`kind = "steps"`).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StepsFile {
    /// The format version of this file; this version of Signal reads 1.
    #[schemars(schema_with = "schema_version")]
    schema_version: Spanned<i64>,
    /// `"steps"`.
    #[schemars(schema_with = "steps_kind")]
    #[allow(dead_code)]
    kind: String,
    /// The number of steps, 1 to 256.
    #[schemars(with = "i64", range(min = 1, max = 256))]
    steps: Spanned<i64>,
    /// The beats per step (0.25 for 16ths), above 0 and at most 64.
    #[schemars(with = "f64", range(min = 0.0, max = 64.0))]
    step: Spanned<f64>,
    /// The lanes, in output order: `name = "x...X..."`, one cell per step
    /// (`.` rest, `x` hit, `X` accent).
    #[schemars(with = "BTreeMap<String, String>")]
    lanes: Spanned<BTreeMap<Spanned<String>, Spanned<String>>>,
    /// The lanes' levels, `name = 0.9`, each from 0 to 1; every key names a
    /// lane, and a lane not listed has level 1.
    #[serde(default)]
    #[schemars(schema_with = "levels")]
    levels: BTreeMap<Spanned<String>, Spanned<f64>>,
}

fn levels(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "additionalProperties": { "type": "number", "minimum": 0.0, "maximum": 1.0 }
    })
}

fn schema_version(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "const": 1 })
}

fn notes_kind(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "string", "const": NOTES })
}

fn steps_kind(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "string", "const": STEPS })
}

/// The kind ([`NOTES`] or [`STEPS`]) of the pattern file `text`, if it is a
/// valid one (`project-format.md` §12).
pub fn pattern_kind(text: &str) -> Option<&'static str> {
    parse(text).ok().map(|p| p.kind)
}

/// Only `kind`, to choose the file's model.
#[derive(Deserialize)]
struct Probe {
    kind: Option<Spanned<toml::Value>>,
}

/// A problem at a byte offset of the file.
pub(crate) type Problem = (usize, String);

/// Reads the pattern file `text`.
pub(crate) fn parse(text: &str) -> Result<Pattern, Vec<Problem>> {
    let syntax = |error: toml::de::Error| {
        vec![(
            error.span().map_or(0, |s| s.start),
            error.message().trim().to_owned(),
        )]
    };
    let probe: Probe = toml::from_str(text).map_err(syntax)?;
    let Some(kind) = probe.kind else {
        return Err(vec![(
            0,
            format!("a pattern file needs `kind`: \"{NOTES}\" or \"{STEPS}\""),
        )]);
    };
    match kind.get_ref().as_str() {
        Some(NOTES) => notes(toml::from_str(text).map_err(syntax)?),
        Some(STEPS) => steps(toml::from_str(text).map_err(syntax)?),
        _ => Err(vec![(
            kind.span().start,
            format!("unknown pattern kind; it is \"{NOTES}\" or \"{STEPS}\""),
        )]),
    }
}

fn check_version(version: &Spanned<i64>, problems: &mut Vec<Problem>) {
    if *version.get_ref() != 1 {
        problems.push((
            version.span().start,
            format!(
                "unsupported schema_version {}; this version of Signal reads 1",
                version.get_ref()
            ),
        ));
    }
}

fn notes(file: NotesFile) -> Result<Pattern, Vec<Problem>> {
    let mut problems = Vec::new();
    check_version(&file.schema_version, &mut problems);
    let length = *file.length.get_ref();
    if !(length.is_finite() && length > 0.0 && length <= MAX_LENGTH) {
        problems.push((
            file.length.span().start,
            format!("length must be above 0 and at most {MAX_LENGTH} beats"),
        ));
    }
    if file.notes.len() > MAX_NOTES {
        problems.push((
            0,
            format!(
                "{} notes; a pattern holds at most {MAX_NOTES}",
                file.notes.len()
            ),
        ));
    }
    // (start, key, length, velocity, offset)
    let mut notes = Vec::new();
    for note in &file.notes {
        let at = note.span().start;
        let note = note.get_ref();
        let key = *note.key.get_ref();
        if !(0..=127).contains(&key) {
            problems.push((
                note.key.span().start,
                "key must be an integer from 0 to 127".to_owned(),
            ));
        }
        let key = key as f64;
        let mut value = |field: &Spanned<f64>, ok: fn(f64) -> bool, what: &str| {
            let v = *field.get_ref();
            if !ok(v) {
                problems.push((field.span().start, what.to_owned()));
            }
            v
        };
        let start = value(
            &note.start,
            |v| v.is_finite() && v >= 0.0,
            "start must be a finite number of beats, at least 0",
        );
        let len = value(
            &note.length,
            |v| v.is_finite() && v > 0.0,
            "length must be a finite number of beats above 0",
        );
        let velocity = value(
            &note.velocity,
            |v| (0.0..=1.0).contains(&v),
            "velocity must be from 0 to 1",
        );
        notes.push((start, key, len, velocity, at));
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    notes.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut last_end: BTreeMap<u64, f64> = BTreeMap::new();
    for &(start, key, len, _, at) in &notes {
        if let Some(&end) = last_end.get(&key.to_bits())
            && start < end
        {
            problems.push((
                at,
                format!("this note overlaps an earlier note on key {key}"),
            ));
        }
        last_end.insert(key.to_bits(), start + len);
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let values = notes
        .iter()
        .flat_map(|&(start, key, len, velocity, _)| [key, start, len, velocity])
        .collect();
    let rows = Table::new(4, values).map_err(|e| vec![(0, e.to_string())])?;
    Ok(Pattern {
        kind: NOTES,
        header: vec![length],
        rows,
    })
}

fn steps(file: StepsFile) -> Result<Pattern, Vec<Problem>> {
    let mut problems = Vec::new();
    check_version(&file.schema_version, &mut problems);
    let steps = *file.steps.get_ref();
    let steps_valid = (1..=MAX_STEPS).contains(&steps);
    if !steps_valid {
        problems.push((
            file.steps.span().start,
            format!("steps must be from 1 to {MAX_STEPS}"),
        ));
    }
    let step = *file.step.get_ref();
    if !(step.is_finite() && step > 0.0 && step <= MAX_STEP) {
        problems.push((
            file.step.span().start,
            format!("step must be above 0 and at most {MAX_STEP} beats"),
        ));
    }
    let lanes_at = file.lanes.span().start;
    let mut lanes: Vec<_> = file.lanes.into_inner().into_iter().collect();
    // File order: TOML tables are maps, the keys' offsets keep the order.
    lanes.sort_by_key(|(name, _)| name.span().start);
    if lanes.is_empty() || lanes.len() > MAX_LANES {
        problems.push((
            lanes_at,
            format!("a step pattern has 1 to {MAX_LANES} lanes"),
        ));
    }
    let width = usize::try_from(steps).unwrap_or(0) + 1;
    for name in file.levels.keys() {
        if !lanes
            .iter()
            .any(|(lane, _)| lane.get_ref() == name.get_ref())
        {
            problems.push((
                name.span().start,
                format!(
                    "`{}` in [levels] is not a lane; every key of [levels] names a lane of [lanes]",
                    name.get_ref()
                ),
            ));
        }
    }
    let mut values = Vec::new();
    for (name, cells) in &lanes {
        if !valid_name(name.get_ref()) {
            problems.push((
                name.span().start,
                format!(
                    "invalid lane name `{}`; names use a-z, 0-9, _ and start with a-z",
                    name.get_ref()
                ),
            ));
        }
        let level = file
            .levels
            .iter()
            .find(|(lane, _)| lane.get_ref() == name.get_ref())
            .map(|(_, level)| level);
        if let Some(level) = level
            && !(0.0..=1.0).contains(level.get_ref())
        {
            problems.push((
                level.span().start,
                format!("level of lane `{}` must be from 0 to 1", name.get_ref()),
            ));
        }
        values.push(level.map_or(1.0, |l| *l.get_ref()));
        let cells_at = cells.span().start;
        let cells: Vec<char> = cells.get_ref().chars().collect();
        if steps_valid && cells.len() as i64 != steps {
            problems.push((
                cells_at,
                format!(
                    "lane `{}` has {} cells; the pattern has {steps} steps",
                    name.get_ref(),
                    cells.len()
                ),
            ));
        }
        for c in cells {
            values.push(match c {
                '.' => 0.0,
                'x' => 1.0,
                'X' => 2.0,
                other => {
                    problems.push((
                        cells_at,
                        format!(
                            "lane `{}`: `{other}` is not a cell; use `.` (rest), `x` (hit) or `X` (accent)",
                            name.get_ref()
                        ),
                    ));
                    0.0
                }
            });
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let rows = Table::new(width, values).map_err(|e| vec![(0, e.to_string())])?;
    Ok(Pattern {
        kind: STEPS,
        header: vec![steps as f64, step],
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_sorted_and_steps_keep_file_order() {
        let notes = parse(
            "schema_version = 1\nkind = \"notes\"\nlength = 4\n\
             notes = [{ key = 64, start = 1, length = 0.5, velocity = 1 },\n\
             { key = 60, start = 0, length = 1, velocity = 0.5 }]\n",
        )
        .unwrap();
        assert_eq!(notes.header, [4.0]);
        assert_eq!(
            notes.rows.values(),
            [60.0, 0.0, 1.0, 0.5, 64.0, 1.0, 0.5, 1.0]
        );
        let steps = parse(
            "schema_version = 1\nkind = \"steps\"\nsteps = 4\nstep = 0.25\n\
             [lanes]\nz = \"x..X\"\na = \".x..\"\n\n[levels]\na = 0.5\n",
        )
        .unwrap();
        assert_eq!(steps.header, [4.0, 0.25]);
        assert_eq!(
            steps.rows.values(),
            [1.0, 1.0, 0.0, 0.0, 2.0, 0.5, 0.0, 1.0, 0.0, 0.0]
        );
    }
}
