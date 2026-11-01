//! The schematic's layout edits: `schematic.move` and `schematic.reset`.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use signal_project::schematic::{self, SCHEMATIC_FILE};
use signal_project::transaction::{self, ProjectLock};

use super::failure::{Failure, fail};
use super::graph::level_of;
use super::{Host, project_ui};

impl Host {
    /// `schematic.move` (ADR 0026 §3): instance `instance` of level `level`
    /// (`""` the top level, else a composite the project uses) at `x`, `y`
    /// (finite, rounded to integers), written into `ui/schematic.toml`. The
    /// batch form, `moves: [{instance, x, y}]` in place of the three, writes
    /// several instances of the level at once (a group dragged together):
    /// all are checked first, and one write holds them all, or none.
    pub(super) fn schematic_move(&mut self, message: &Value) -> Result<Value, Failure> {
        let level = message
            .get("level")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("bad-message", "schematic.move needs `level`"))?;
        let entry = |item: &Value| -> Result<(String, i64, i64), Failure> {
            let instance = item
                .get("instance")
                .and_then(Value::as_str)
                .ok_or_else(|| fail("bad-message", "schematic.move needs `instance`"))?;
            let coordinate = |key: &str| {
                item.get(key)
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite() && v.abs() <= 1e9)
                    .map(|v| v.round() as i64)
                    .ok_or_else(|| {
                        fail(
                            "bad-value",
                            format!("`{key}` must be a finite number, at most 1e9"),
                        )
                    })
            };
            Ok((instance.to_owned(), coordinate("x")?, coordinate("y")?))
        };
        let batch = message.get("moves");
        let moves = match batch {
            None => vec![entry(message)?],
            Some(Value::Array(items))
                if !items.is_empty()
                    && ["instance", "x", "y"]
                        .iter()
                        .all(|key| message.get(key).is_none()) =>
            {
                items.iter().map(entry).collect::<Result<Vec<_>, _>>()?
            }
            Some(_) => {
                return Err(fail(
                    "bad-message",
                    "`moves` is a non-empty list of {instance, x, y}, in place of `instance`, `x` and `y` (neither may be given beside it)",
                ));
            }
        };
        let mut named = std::collections::BTreeSet::new();
        if let Some((instance, _, _)) = moves
            .iter()
            .find(|(instance, _, _)| !named.insert(instance.as_str()))
        {
            return Err(fail(
                "bad-value",
                format!("`moves` names instance `{instance}` twice"),
            ));
        }
        let graph = self.graph()?;
        let instances = level_of(&graph, level)?["instances"].as_array();
        for (instance, _, _) in &moves {
            if !instances.is_some_and(|all| all.iter().any(|i| i["name"] == instance.as_str())) {
                let at = if level.is_empty() {
                    "the top level".to_owned()
                } else {
                    format!("composite {level}")
                };
                return Err(fail(
                    "unknown-instance",
                    format!("{at} has no instance `{instance}`"),
                ));
            }
        }
        let entries: Vec<(&str, i64, i64)> = moves
            .iter()
            .map(|(instance, x, y)| (instance.as_str(), *x, *y))
            .collect();
        self.write_schematic(|text| schematic::set_positions(text, level, &entries))?;
        if let (None, [(instance, x, y)]) = (batch, moves.as_slice()) {
            return Ok(json!({
                "type": "schematic.moved",
                "level": level,
                "instance": instance,
                "x": x,
                "y": y,
                "file": SCHEMATIC_FILE,
            }));
        }
        let moved: Vec<Value> = moves
            .iter()
            .map(|(instance, x, y)| json!({"instance": instance, "x": x, "y": y}))
            .collect();
        Ok(json!({
            "type": "schematic.moved",
            "level": level,
            "moves": moved,
            "file": SCHEMATIC_FILE,
        }))
    }

    /// `schematic.reset`: removes the table of `level`, or without one the
    /// whole `ui/schematic.toml`.
    pub(super) fn schematic_reset(&mut self, message: &Value) -> Result<Value, Failure> {
        let path = self.dir.join(SCHEMATIC_FILE);
        match message.get("level") {
            None | Some(Value::Null) => {
                self.schematic_unlinked()?;
                let _lock = ProjectLock::acquire(&self.dir)
                    .map_err(|e| fail("write-failed", format!("{}: {e}", path.display())))?;
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(fail("write-failed", format!("{}: {error}", path.display())));
                    }
                }
                self.seen(Path::new(SCHEMATIC_FILE));
                Ok(json!({"type": "schematic.reset", "level": null, "file": SCHEMATIC_FILE}))
            }
            Some(Value::String(level)) => {
                let graph = self.graph()?;
                level_of(&graph, level)?;
                if path.exists() {
                    self.write_schematic(|text| schematic::remove_level(text, level))?;
                }
                Ok(json!({"type": "schematic.reset", "level": level, "file": SCHEMATIC_FILE}))
            }
            Some(_) => Err(fail("bad-message", "`level` is a string or null")),
        }
    }

    /// `ui/schematic.toml` edited by `edit`, written atomically as `commit`
    /// writes `project.toml`, and taken as seen by the watch (the write's
    /// `graph` is pushed at once instead).
    fn write_schematic(
        &mut self,
        edit: impl Fn(&str) -> Result<String, String>,
    ) -> Result<(), Failure> {
        self.schematic_unlinked()?;
        let path = self.dir.join(SCHEMATIC_FILE);
        let failed = |error: String| {
            fail(
                "write-failed",
                format!("{} was not written: {error}", path.display()),
            )
        };
        let _lock = ProjectLock::acquire(&self.dir).map_err(|e| failed(e.to_string()))?;
        // Edited again if it changes before the rename (ADR 0040).
        transaction::edit_text(&path, |before| edit(before).map(Some)).map_err(failed)?;
        self.counts.2 += 1;
        self.seen(Path::new(SCHEMATIC_FILE));
        Ok(())
    }

    /// `ui/schematic.toml` is the project's own file: neither it nor `ui/`
    /// is a link (a linked `ui/` is served, not edited, as for
    /// [`Self::panel_path`]). `write-failed` otherwise, before anything is
    /// written.
    pub(super) fn schematic_unlinked(&self) -> Result<(), Failure> {
        let link =
            |path: &Path| fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        if link(&project_ui(&self.dir)) || link(&self.dir.join(SCHEMATIC_FILE)) {
            return Err(fail(
                "write-failed",
                format!(
                    "{SCHEMATIC_FILE} was not written: it is reached through a link, and the positions are written only into the project's own files"
                ),
            ));
        }
        Ok(())
    }
}
