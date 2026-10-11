//! Diagnostics: an error or warning at a file, line and column.

use std::fmt;
use std::path::Path;

use serde::Serialize;

/// Whether a diagnostic prevents loading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The project cannot be loaded or planned.
    Error,
    /// The project loads; something is probably unintended.
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        })
    }
}

/// An error or warning in a project file, written
/// `file:line:column: severity: message`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    /// Error or warning.
    pub severity: Severity,
    /// The file, relative to the project folder with `/` separators (e.g.
    /// `composites/voice.toml`).
    pub file: String,
    /// The line, from 1.
    pub line: usize,
    /// The column in characters, from 1.
    pub column: usize,
    /// What is wrong.
    pub message: String,
}

impl Diagnostic {
    /// A diagnostic at byte `offset` of `text`, the content of `file`, with
    /// its 1-based line and character column.
    pub fn at(
        severity: Severity,
        file: &str,
        text: &str,
        offset: usize,
        message: String,
    ) -> Diagnostic {
        let (line, column) = line_column(text, offset);
        Diagnostic {
            severity,
            file: file.to_owned(),
            line,
            column,
            message,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}: {}",
            self.file, self.line, self.column, self.severity, self.message
        )
    }
}

/// Diagnostics with their file paths joined to the project folder `dir`.
pub fn located(dir: &Path, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .map(|d| Diagnostic {
            file: dir.join(&d.file).display().to_string(),
            ..d
        })
        .collect()
}

/// The 1-based line and character column of byte `offset` in `text` (clamped
/// to the text, and to a character boundary).
fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = text.get(..offset).unwrap_or("");
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::line_column;

    #[test]
    fn line_and_column_are_one_based_in_characters() {
        let text = "ab\nçd\n";
        assert_eq!(line_column(text, 0), (1, 1));
        assert_eq!(line_column(text, 1), (1, 2));
        assert_eq!(line_column(text, 3), (2, 1));
        // `ç` is two bytes but one column.
        assert_eq!(line_column(text, 5), (2, 2));
        assert_eq!(line_column(text, 4), (2, 1));
        assert_eq!(line_column(text, 99), (3, 1));
    }
}
