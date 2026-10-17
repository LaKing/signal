//! Instance tables (ADR 0028 §4): an immutable table of `f64` rows that a
//! library compiles from project data when a plan is built, and that the
//! engine hands to one instance in every call ([`Inputs::table`](crate::Inputs::table)).
//!
//! Domain-free: the core knows rows of numbers only. What the rows mean, and
//! which data a type takes ([`TableRule::kind`]), is the library's.

use std::fmt;
use std::sync::Arc;

/// The most values one table holds (8 MiB of `f64`): no project text makes a
/// plan allocate without bound.
pub const MAX_TABLE_VALUES: usize = 1 << 20;

/// An immutable table of rows of `width` finite `f64` values each.
///
/// Cloning shares the values (a reference count); building one allocates, so
/// tables are made off the audio thread, when a plan is built.
#[derive(Clone, PartialEq)]
pub struct Table {
    width: usize,
    values: Arc<[f64]>,
}

impl Table {
    /// A table of rows of `width` values, row after row. Refuses a width of
    /// 0, a length that is not a whole number of rows, more than
    /// [`MAX_TABLE_VALUES`] values, and a value that is not finite.
    pub fn new(width: usize, values: Vec<f64>) -> Result<Table, TableError> {
        if width == 0 {
            return Err(TableError::ZeroWidth);
        }
        if values.len() > MAX_TABLE_VALUES {
            return Err(TableError::TooLarge(values.len()));
        }
        if !values.len().is_multiple_of(width) {
            return Err(TableError::PartialRow {
                width,
                len: values.len(),
            });
        }
        if let Some(index) = values.iter().position(|v| !v.is_finite()) {
            return Err(TableError::NotFinite(index));
        }
        Ok(Table {
            width,
            values: values.into(),
        })
    }

    /// The values in each row (≥ 1).
    pub fn width(&self) -> usize {
        self.width
    }

    /// The number of rows.
    pub fn rows(&self) -> usize {
        self.values.len() / self.width
    }

    /// Row `index`, or an empty slice past the last row. Never panics.
    pub fn row(&self, index: usize) -> &[f64] {
        let start = index.saturating_mul(self.width);
        let end = start.saturating_add(self.width);
        self.values.get(start..end).unwrap_or(&[])
    }

    /// Every value, row after row.
    pub fn values(&self) -> &[f64] {
        &self.values
    }

    /// Whether both have the same width and bit-identical values: a table
    /// that is not identical to an instance's current one is a live edit.
    pub fn identical(&self, other: &Table) -> bool {
        self.width == other.width
            && (Arc::ptr_eq(&self.values, &other.values)
                || (self.values.len() == other.values.len()
                    && self
                        .values
                        .iter()
                        .zip(other.values.iter())
                        .all(|(a, b)| a.to_bits() == b.to_bits())))
    }
}

impl fmt::Debug for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Table")
            .field("width", &self.width)
            .field("rows", &self.rows())
            .finish()
    }
}

/// Why a [`Table`] could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableError {
    /// A width of 0.
    ZeroWidth,
    /// More than [`MAX_TABLE_VALUES`] values.
    TooLarge(usize),
    /// A length that is not a whole number of rows.
    PartialRow {
        /// The width.
        width: usize,
        /// The number of values.
        len: usize,
    },
    /// The value at this index is NaN or infinite.
    NotFinite(usize),
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableError::ZeroWidth => write!(f, "a table's rows need at least one value"),
            TableError::TooLarge(len) => write!(
                f,
                "a table holds at most {MAX_TABLE_VALUES} values, not {len}"
            ),
            TableError::PartialRow { width, len } => {
                write!(f, "{len} values are not whole rows of {width}")
            }
            TableError::NotFinite(index) => write!(f, "value {index} is not finite"),
        }
    }
}

impl std::error::Error for TableError {}

/// What a module type takes as its table (ADR 0028 §3, §4), registered with
/// [`Registry::set_table_rule`](crate::Registry::set_table_rule).
///
/// The project format reads the data `kind` names into a *source*: a header
/// and a table in the format's canonical layout. `compile` turns that into
/// the table the instance reads, or says why it cannot (a load error at the
/// instance). Both are the library's vocabulary; the core passes numbers.
#[derive(Clone, Copy, Debug)]
pub struct TableRule {
    /// The kind of data the type takes, e.g. `"notes"`.
    pub kind: &'static str,
    /// Compiles `(header, rows)` into the instance's table. Not real time.
    pub compile: fn(header: &[f64], rows: &Table) -> Result<Table, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_whole_finite_rows() {
        let table = Table::new(2, vec![1.0, 2.0, 3.0, 4.0]).unwrap();
        assert_eq!((table.width(), table.rows()), (2, 2));
        assert_eq!(table.row(1), [3.0, 4.0]);
        assert!(table.row(2).is_empty());
        assert_eq!(Table::new(0, vec![]), Err(TableError::ZeroWidth));
        assert_eq!(
            Table::new(2, vec![1.0]),
            Err(TableError::PartialRow { width: 2, len: 1 })
        );
        assert_eq!(Table::new(1, vec![f64::NAN]), Err(TableError::NotFinite(0)));
        assert!(Table::new(1, vec![0.0; MAX_TABLE_VALUES + 1]).is_err());
    }

    #[test]
    fn identity_is_bitwise() {
        let a = Table::new(1, vec![0.0]).unwrap();
        assert!(a.identical(&a.clone()));
        assert!(a.identical(&Table::new(1, vec![0.0]).unwrap()));
        assert!(!a.identical(&Table::new(1, vec![-0.0]).unwrap()));
        assert!(!a.identical(&Table::new(1, vec![0.0, 0.0]).unwrap()));
    }
}
