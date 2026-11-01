//! The project's structure: `describe`, `graph`, `modules` and their pushes.

use serde_json::Value;

use super::failure::{Failure, fail};

/// Level `level` of a `graph` message: `""` the top level, else a composite.
pub(super) fn level_of<'a>(graph: &'a Value, level: &str) -> Result<&'a Value, Failure> {
    let found = if level.is_empty() {
        graph.get("top")
    } else {
        graph.get("composites").and_then(|c| c.get(level))
    };
    found.ok_or_else(|| {
        fail(
            "unknown-level",
            format!("the project uses no composite `{level}`"),
        )
    })
}
