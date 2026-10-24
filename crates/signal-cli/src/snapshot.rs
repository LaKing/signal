//! `signal snapshot`: a project's snapshots (ADR 0041) from its files
//! alone: list, save, restore and delete. Save records each control's value
//! as the files hold it (the default without a key); restore writes the
//! snapshot's values into `project.toml` as one change, which a running
//! Signal applies through its watch. The work is `signal_project::snapshot`
//! (the file) and `signal_ui::snapshot` (which parameters).

use std::path::PathBuf;

use clap::{Args, Subcommand};
use serde_json::json;
use signal_core::Registry;
use signal_project::snapshot::{self, SnapshotError};
use signal_ui::snapshot::{RestoreError, planned_panel, restore_files, values};

use crate::{Failure, print_stdout};

/// Arguments of `signal snapshot`.
#[derive(Args)]
pub(crate) struct SnapshotArgs {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// List the snapshots, with how many values each holds.
    List {
        /// The project folder.
        project: PathBuf,
    },
    /// Save every top-level parameter's value, as the files hold it, under a name.
    Save {
        /// The project folder.
        project: PathBuf,
        /// The snapshot's name (1 to 64 characters, trimmed).
        name: String,
        /// Replace a snapshot of that name.
        #[arg(long)]
        overwrite: bool,
    },
    /// Write a snapshot's values back into project.toml, as one change.
    Restore {
        /// The project folder.
        project: PathBuf,
        /// The snapshot's name.
        name: String,
    },
    /// Delete a snapshot.
    Delete {
        /// The project folder.
        project: PathBuf,
        /// The snapshot's name.
        name: String,
    },
}

fn failure(error: SnapshotError) -> Failure {
    match error {
        SnapshotError::BadName(_) | SnapshotError::Unknown(_) | SnapshotError::Exists(_) => {
            Failure::Usage(error.to_string())
        }
        _ => Failure::Io(error.to_string()),
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
                print_stdout(&json!({"snapshots": list}).to_string());
            } else {
                for (name, values) in &all.snapshots {
                    print_stdout(&format!("{name} ({} values)", values.len()));
                }
            }
            Ok(())
        }
        Action::Save {
            project,
            name,
            overwrite,
        } => {
            let panel = planned_panel(project, registry).map_err(|e| match e {
                RestoreError::DoesNotLoad(message) => Failure::Io(message),
                RestoreError::Write(error) => Failure::Io(error.to_string()),
            })?;
            let values = values(&panel, |_, p| p.base);
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
            let (all, _) = snapshot::read(project).map_err(failure)?;
            let values = all
                .snapshots
                .get(&name)
                .ok_or_else(|| failure(SnapshotError::Unknown(name.clone())))?;
            let restored = restore_files(project, registry, values).map_err(|e| match e {
                RestoreError::DoesNotLoad(message) => Failure::Io(message),
                RestoreError::Write(error) => Failure::Io(error.to_string()),
            })?;
            let restore = &restored.restore;
            if json {
                print_stdout(
                    &json!({
                        "name": name,
                        "restored": restore.edits.len(),
                        "written": restored.written,
                        "skipped": restore.skipped.iter().map(|s| json!({"address": s.address, "reason": s.reason})).collect::<Vec<_>>(),
                        "clamped": restore.clamped.iter().map(|c| json!({"address": c.address, "value": c.value, "to": c.to})).collect::<Vec<_>>(),
                    })
                    .to_string(),
                );
            } else {
                print_stdout(&format!(
                    "restored \"{name}\": {} values, {} written into project.toml",
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
    }
}
