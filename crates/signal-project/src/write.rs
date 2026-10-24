//! Parameter write-back (ADR 0009, 0013): a top-level instance's parameter
//! value written into `project.toml` with `toml_edit`, so that only that
//! value's text changes and comments, ordering and spacing are kept.
//! Control-thread code: it works on text, the caller reads and writes files.

use std::fmt;

use toml_edit::{DocumentMut, InlineTable, Item, Key, Table, Value};

use crate::{Diagnostic, PROJECT_FILE, Project};

/// Why a parameter could not be written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// The text is not valid TOML.
    Syntax(String),
    /// `[instances]` has no such instance, or it is not a table.
    NoInstance(String),
    /// The value is not finite.
    NotFinite(String),
    /// The project with the value written does not load.
    Invalid(Vec<Diagnostic>),
    /// The project includes others (ADR 0035): the running project does not
    /// take the write, and the watch's reload of the folder applies it.
    Includes,
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Syntax(message) => write!(f, "project.toml does not parse: {message}"),
            WriteError::NoInstance(instance) => {
                write!(
                    f,
                    "project.toml has no instance table `{instance}` in [instances]"
                )
            }
            WriteError::NotFinite(parameter) => {
                write!(f, "{parameter}: a parameter value must be finite")
            }
            WriteError::Invalid(diagnostics) => {
                for (i, diagnostic) in diagnostics.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{diagnostic}")?;
                }
                Ok(())
            }
            WriteError::Includes => write!(
                f,
                "the project includes others, so the reload of its folder applies the write"
            ),
        }
    }
}

impl std::error::Error for WriteError {}

/// `text`, a `project.toml`, with `parameter` of the top-level `instance` set
/// to `value`, or removed (`None`: back to its default).
///
/// An existing value's text is replaced, keeping the spacing around it. A
/// key not in the file is inserted after `module` and the parameters that
/// come before it in `order` (the module's parameters in declared order), so
/// its position does not depend on which keys were written first. Nothing
/// else in the text changes. A value is written in the shortest form that
/// reads back to the same `f64`, as an integer when it is one.
pub fn write_parameter(
    text: &str,
    instance: &str,
    parameter: &str,
    value: Option<f64>,
    order: &[&str],
) -> Result<String, WriteError> {
    let mut document: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| WriteError::Syntax(e.message().to_owned()))?;
    let value = match value {
        Some(v) if !v.is_finite() => return Err(WriteError::NotFinite(parameter.to_owned())),
        Some(v) => Some(number(v)),
        None => None,
    };
    let entry = document
        .get_mut("instances")
        .and_then(Item::as_table_like_mut)
        .and_then(|instances| instances.get_mut(instance))
        .ok_or_else(|| WriteError::NoInstance(instance.to_owned()))?;
    match entry {
        Item::Value(Value::InlineTable(table)) => inline(table, parameter, value, order),
        Item::Table(table) => standard(table, parameter, value),
        _ => return Err(WriteError::NoInstance(instance.to_owned())),
    }
    Ok(document.to_string())
}

/// `text`, a `project.toml`, with `parameter` of the top-level `instance`
/// left out of snapshots (`excluded`) or put back in (ADR 0041): its name
/// added to or removed from the entry's `snapshot_exclude` list, kept in
/// byte order, the key removed when the list is empty (absent: every
/// control goes in). Nothing else in the text changes; `None` when it already
/// says so.
pub fn write_snapshot_exclude(
    text: &str,
    instance: &str,
    parameter: &str,
    excluded: bool,
) -> Result<Option<String>, WriteError> {
    const KEY: &str = "snapshot_exclude";
    let mut document: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| WriteError::Syntax(e.message().to_owned()))?;
    let entry = document
        .get_mut("instances")
        .and_then(Item::as_table_like_mut)
        .and_then(|instances| instances.get_mut(instance))
        .ok_or_else(|| WriteError::NoInstance(instance.to_owned()))?;
    let current = |table: &dyn toml_edit::TableLike| -> Vec<String> {
        table
            .get(KEY)
            .and_then(Item::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut names = match &*entry {
        Item::Value(Value::InlineTable(table)) => current(table),
        Item::Table(table) => current(table),
        _ => return Err(WriteError::NoInstance(instance.to_owned())),
    };
    if names.iter().any(|n| n == parameter) == excluded {
        return Ok(None);
    }
    if excluded {
        names.push(parameter.to_owned());
        names.sort();
    } else {
        names.retain(|n| n != parameter);
    }
    let value = (!names.is_empty()).then(|| {
        let mut array = toml_edit::Array::new();
        for name in &names {
            array.push(name.as_str());
        }
        Value::Array(array)
    });
    match entry {
        Item::Value(Value::InlineTable(table)) => {
            // Last in the entry: after every key it has.
            let mut order: Vec<String> = table.iter().map(|(k, _)| k.to_owned()).collect();
            order.retain(|k| k != KEY);
            order.push(KEY.to_owned());
            let order: Vec<&str> = order.iter().map(String::as_str).collect();
            inline(table, KEY, value, &order);
        }
        Item::Table(table) => standard(table, KEY, value),
        _ => return Err(WriteError::NoInstance(instance.to_owned())),
    }
    Ok(Some(document.to_string()))
}

/// The value `text` (a `project.toml`) sets for `parameter` of the
/// top-level `instance`: `None` when the entry has no such key (or the text
/// does not read), so the parameter is at its default.
pub fn read_parameter(text: &str, instance: &str, parameter: &str) -> Option<f64> {
    let document: DocumentMut = text.parse().ok()?;
    let value = document
        .get("instances")?
        .as_table_like()?
        .get(instance)?
        .as_table_like()?
        .get(parameter)?
        .as_value()?
        .clone();
    match value {
        Value::Integer(i) => Some(*i.value() as f64),
        Value::Float(f) => Some(*f.value()),
        _ => None,
    }
}

/// `text` with every edit of `edits` applied in order by [`write_parameter`]:
/// `(instance, parameter, value, order)`. One text, so a caller writes the
/// edits as one change (a snapshot's restore, ADR 0041).
pub fn write_parameters(
    text: &str,
    edits: &[(&str, &str, Option<f64>, &[&str])],
) -> Result<String, WriteError> {
    let mut text = text.to_owned();
    for (instance, parameter, value, order) in edits {
        text = write_parameter(&text, instance, parameter, *value, order)?;
    }
    Ok(text)
}

impl Project {
    /// This project with [`write_parameter`] applied to its `project.toml`:
    /// what the running project becomes when a value is written back, so
    /// that the [`diff`](crate::diff) between it and the folder reloaded
    /// after the write is empty (nothing is applied twice).
    pub fn with_parameter(
        &self,
        instance: &str,
        parameter: &str,
        value: Option<f64>,
        order: &[&str],
    ) -> Result<Project, WriteError> {
        self.with_parameters(&[(instance, parameter, value, order)])
    }

    /// This project with `text` as its `project.toml` (an edit that changes
    /// nothing that plays, such as `snapshot_exclude`, ADR 0041), refused
    /// for a project that includes others as [`Project::with_parameter`]
    /// is.
    pub fn with_project_text(&self, text: &str) -> Result<Project, WriteError> {
        if !self.content.projects.is_empty() || !self.content.files.is_empty() {
            return Err(WriteError::Includes);
        }
        let mut files = self.files.clone();
        for file in files.iter_mut().filter(|f| f.path == PROJECT_FILE) {
            text.clone_into(&mut file.text);
        }
        Project::from_sources(files).map_err(WriteError::Invalid)
    }

    /// [`Project::with_parameter`] for several edits at once
    /// ([`write_parameters`]).
    pub fn with_parameters(
        &self,
        edits: &[(&str, &str, Option<f64>, &[&str])],
    ) -> Result<Project, WriteError> {
        let mut files = self.files.clone();
        for file in files.iter_mut().filter(|f| f.path == PROJECT_FILE) {
            file.text = write_parameters(&file.text, edits)?;
        }
        // A project that includes others is not rebuilt here: read from its
        // folder again, an included project changed on disk but not yet
        // reloaded would be taken as running, and the reload that follows
        // would find no difference and never apply it. The watch's reload
        // applies the write instead.
        if !self.content.projects.is_empty() || !self.content.files.is_empty() {
            return Err(WriteError::Includes);
        }
        Project::from_sources(files).map_err(WriteError::Invalid)
    }
}

/// `value` as a TOML number that reads back to it bit for bit.
pub(crate) fn number(value: f64) -> Value {
    // −0.0 stays a float: the integer 0 would read back as +0.0.
    if value.fract() == 0.0 && value.abs() < 1e15 && value.to_bits() != (-0.0f64).to_bits() {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

pub(crate) fn inline(
    table: &mut InlineTable,
    parameter: &str,
    value: Option<Value>,
    order: &[&str],
) {
    let keys: Vec<String> = table.iter().map(|(k, _)| k.to_owned()).collect();
    let last = keys.last().cloned();
    let Some(value) = value else {
        // Removed: the spacing before `}` stays with the new last entry.
        if let Some(removed) = table.remove(parameter)
            && last.as_deref() == Some(parameter)
            && let Some((_, now_last)) = table.iter_mut().last()
        {
            let suffix = removed.decor().suffix().cloned();
            now_last.decor_mut().set_suffix(suffix.unwrap_or_default());
        }
        return;
    };
    if let Some(old) = table.get_mut(parameter) {
        let decor = old.decor().clone();
        *old = value;
        *old.decor_mut() = decor;
        return;
    }
    // After `module` and every key declared before `parameter`.
    let rank = |key: &str| order.iter().position(|o| *o == key);
    let new = rank(parameter);
    let at = keys
        .iter()
        .rposition(|k| k == "module" || matches!((rank(k), new), (Some(r), Some(n)) if r < n))
        .map_or(0, |i| i + 1);
    let tail: Vec<(Key, Value)> = keys
        .get(at..)
        .unwrap_or_default()
        .iter()
        .filter_map(|k| table.remove_entry(k))
        .collect();
    let mut value = value.decorated(" ", "");
    if tail.is_empty() {
        // Appended: it takes over the spacing before `}`.
        if let Some((_, before)) = table.iter_mut().last() {
            let suffix = before.decor().suffix().cloned().unwrap_or_default();
            before.decor_mut().set_suffix("");
            value.decor_mut().set_suffix(suffix);
        }
    }
    let key = Key::new(parameter).with_leaf_decor(toml_edit::Decor::new(" ", " "));
    table.insert_formatted(&key, value);
    for (key, value) in tail {
        table.insert_formatted(&key, value);
    }
}

pub(crate) fn standard(table: &mut Table, parameter: &str, value: Option<Value>) {
    match value {
        None => {
            table.remove(parameter);
        }
        Some(value) => match table.get_mut(parameter).and_then(Item::as_value_mut) {
            Some(old) => {
                let decor = old.decor().clone();
                *old = value;
                *old.decor_mut() = decor;
            }
            None => {
                table.insert(parameter, Item::Value(value));
            }
        },
    }
}
