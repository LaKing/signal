//! `signal snapshot`: a project's snapshots (ADR 0041) from its files
//! alone: list, save, restore, undo, delete, and which controls go into
//! them (exclude). Save records each control's value as the files hold it
//! (the default without a key); restore first keeps the values it replaces
//! as the undo snapshot, then writes the snapshot's values into
//! `project.toml` as one change, which a running Signal applies through its
//! watch. The work is `signal_project::snapshot` (the file) and
//! `signal_ui::snapshot` (which parameters).

use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde_json::json;
use signal_core::Registry;
use signal_project::snapshot::{self, SnapshotError, Values};
use signal_project::transaction::{ProjectLock, replace_unchanged};
use signal_ui::panel::Panel;
use signal_ui::snapshot::{RestoreError, Restored, planned_panel, restore_files, values};

use crate::{Failure, print_stdout};

/// Arguments of `signal snapshot`.
#[derive(Args)]
pub(crate) struct SnapshotArgs {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// List the snapshots, with how many values each holds, and whether
    /// there is an undo snapshot.
    List {
        /// The project folder.
        project: PathBuf,
    },
    /// Save every top-level parameter's value, as the files hold it, under
    /// a name (the controls left out of snapshots are not saved).
    Save {
        /// The project folder.
        project: PathBuf,
        /// The snapshot's name (1 to 64 characters, trimmed).
        name: String,
        /// Replace a snapshot of that name.
        #[arg(long)]
        overwrite: bool,
    },
    /// Write a snapshot's values back into project.toml, as one change; the
    /// values it replaces become the undo snapshot.
    Restore {
        /// The project folder.
        project: PathBuf,
        /// The snapshot's name.
        name: String,
    },
    /// Restore the undo snapshot (what the last restore replaced); the
    /// values it replaces become the undo snapshot, so a second undo redoes.
    Undo {
        /// The project folder.
        project: PathBuf,
    },
    /// Delete a snapshot.
    Delete {
        /// The project folder.
        project: PathBuf,
        /// The snapshot's name.
        name: String,
    },
    /// Leave a control out of snapshots (its name in its instance's
    /// `snapshot_exclude` in project.toml), or put it back with --include.
    Exclude {
        /// The project folder.
        project: PathBuf,
        /// The control's address, `/instance.parameter`.
        address: String,
        /// Put it back into snapshots.
        #[arg(long)]
        include: bool,
    },
}

fn failure(error: SnapshotError) -> Failure {
    match error {
        SnapshotError::BadName(_)
        | SnapshotError::Unknown(_)
        | SnapshotError::Exists(_)
        | SnapshotError::NoUndo => Failure::Usage(error.to_string()),
        _ => Failure::Io(error.to_string()),
    }
}

fn restore_failure(error: RestoreError) -> Failure {
    match error {
        RestoreError::DoesNotLoad(message) => Failure::Io(message),
        RestoreError::Write(error) => Failure::Io(error.to_string()),
    }
}

/// The panel of `project`, refused when it does not plan.
fn panel(project: &Path, registry: &Registry) -> Result<Panel, Failure> {
    planned_panel(project, registry).map_err(restore_failure)
}

/// Restores what `take` gives from the snapshots (a named one, or the undo
/// snapshot) after the files' values of every control in snapshots become
/// the undo snapshot, in the same write; then writes them into
/// project.toml.
fn restore_with(
    project: &Path,
    registry: &Registry,
    take: impl FnOnce(&mut snapshot::Snapshots) -> Result<Values, SnapshotError>,
) -> Result<Restored, Failure> {
    let now = values(&panel(project, registry)?, |_, p| p.base);
    let (values, ..) = snapshot::edit(
        project,
        |_| Ok(()),
        |all| {
            let values = take(all)?;
            all.undo = Some(now);
            Ok(values)
        },
    )
    .map_err(failure)?;
    restore_files(project, registry, &values).map_err(restore_failure)
}

fn report(name: &str, restored: &Restored, json: bool) {
    let restore = &restored.restore;
    if json {
        print_stdout(
            &json!({
                "name": name,
                "restored": restore.edits.len(),
                "written": restored.written,
                "skipped": restore.skipped.iter().map(|s| json!({"address": s.address, "reason": s.reason})).collect::<Vec<_>>(),
                "clamped": restore.clamped.iter().map(|c| json!({"address": c.address, "value": c.value, "to": c.to})).collect::<Vec<_>>(),
                "undo": true,
            })
            .to_string(),
        );
        return;
    }
    print_stdout(&format!(
        "restored \"{name}\": {} values, {} written into project.toml (the values it replaced are the undo snapshot)",
        restore.edits.len(),
        restored.written.len()
    ));
    for s in &restore.skipped {
        print_stdout(&format!("skipped {}: {}", s.address, s.reason));
    }
    for c in &restore.clamped {
        print_stdout(&format!(
            "clamped {} from {} to {} (its range)",
            c.address, c.value, c.to
        ));
    }
}

/// `signal snapshot`.
pub(crate) fn run(args: &SnapshotArgs, registry: &Registry, json: bool) -> Result<(), Failure> {
    match &args.action {
        Action::List { project } => {
            let (all, _) = snapshot::read(project).map_err(failure)?;
            if json {
                let list: Vec<_> = all
                    .snapshots
                    .iter()
                    .map(|(name, values)| json!({"name": name, "count": values.len(), "values": values}))
                    .collect();
                print_stdout(
                    &json!({
                        "snapshots": list,
                        "undo": all.undo.is_some(),
                        "undoCount": all.undo.as_ref().map_or(0, |u| u.len()),
                    })
                    .to_string(),
                );
            } else {
                for (name, values) in &all.snapshots {
                    print_stdout(&format!("{name} ({} values)", values.len()));
                }
                if let Some(undo) = &all.undo {
                    print_stdout(&format!(
                        "(undo snapshot: {} values; `signal snapshot undo` restores it)",
                        undo.len()
                    ));
                }
            }
            Ok(())
        }
        Action::Save {
            project,
            name,
            overwrite,
        } => {
            let values = values(&panel(project, registry)?, |_, p| p.base);
            let count = values.len();
            let (replaced, ..) =
                snapshot::save(project, name, values, *overwrite, |_| Ok(())).map_err(failure)?;
            let name = snapshot::name(name).map_err(failure)?;
            if json {
                print_stdout(
                    &json!({"name": name, "count": count, "replaced": replaced}).to_string(),
                );
            } else {
                let verb = if replaced { "replaced" } else { "saved" };
                print_stdout(&format!("{verb} \"{name}\" ({count} values)"));
            }
            Ok(())
        }
        Action::Restore { project, name } => {
            let name = snapshot::name(name).map_err(failure)?;
            let restored = restore_with(project, registry, |all| {
                all.snapshots
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| SnapshotError::Unknown(name.clone()))
            })?;
            report(&name, &restored, json);
            Ok(())
        }
        Action::Undo { project } => {
            let restored = restore_with(project, registry, |all| {
                all.undo.take().ok_or(SnapshotError::NoUndo)
            })?;
            report("undo", &restored, json);
            Ok(())
        }
        Action::Delete { project, name } => {
            let (values, ..) = snapshot::delete(project, name, |_| Ok(())).map_err(failure)?;
            let name = snapshot::name(name).map_err(failure)?;
            if json {
                print_stdout(&json!({"name": name, "count": values.len()}).to_string());
            } else {
                print_stdout(&format!("deleted \"{name}\""));
            }
            Ok(())
        }
        Action::Exclude {
            project,
            address,
            include,
        } => {
            let control = panel(project, registry)?;
            let (instance, parameter) = address
                .strip_prefix('/')
                .and_then(|a| a.split_once('.'))
                .filter(|(i, _)| !i.contains('/'))
                .filter(|(i, p)| control.target(i, p).is_some())
                .ok_or_else(|| {
                    Failure::Usage(format!(
                        "{address} is no control: only a top-level instance's parameters go into snapshots"
                    ))
                })?;
            let path = project.join(signal_project::PROJECT_FILE);
            let io = |e: String| Failure::Io(format!("{}: {e}", path.display()));
            let _lock = ProjectLock::acquire(project).map_err(|e| io(e.to_string()))?;
            let before = fs::read_to_string(&path).map_err(|e| io(e.to_string()))?;
            let after =
                signal_project::write_snapshot_exclude(&before, instance, parameter, !include)
                    .map_err(|e| io(e.to_string()))?;
            if let Some(after) = &after
                && !replace_unchanged(&path, Some(before.as_bytes()), after.as_bytes())
                    .map_err(|e| io(e.to_string()))?
            {
                return Err(io(
                    "it changed meanwhile (another writer); nothing was written".to_owned(),
                ));
            }
            if json {
                print_stdout(
                    &json!({"address": address, "inSnapshots": *include, "written": after.is_some()})
                        .to_string(),
                );
            } else {
                let state = if *include {
                    "goes into"
                } else {
                    "is left out of"
                };
                print_stdout(&format!("{address} {state} snapshots"));
            }
            Ok(())
        }
    }
}
