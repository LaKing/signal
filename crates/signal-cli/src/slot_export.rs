//! `signal slot-export`: a top-level slot saved as a project of its own
//! (ADR 0046 §9, ADR 0051 §10; the work is
//! `signal_project::export::export_slot`).

use std::path::PathBuf;

use clap::Args;
use serde_json::json;
use signal_project::Project;

use crate::{Failure, print_stdout};

/// Arguments of `signal slot-export`.
#[derive(Args)]
pub(crate) struct SlotExportArgs {
    /// The project folder.
    project: PathBuf,
    /// The top-level slot.
    slot: String,
    /// The new project's folder: it must not exist, or be empty.
    #[arg(long)]
    out: PathBuf,
}

/// `signal slot-export`.
pub(crate) fn run(args: &SlotExportArgs, json: bool) -> Result<(), Failure> {
    let project = Project::load(&args.project).map_err(|e| Failure::Usage(e.to_string()))?;
    let part_path = args
        .project
        .join("ui/parts")
        .join(format!("{}.html", args.slot));
    let part = signal_project::read_regular_to_string(&part_path).ok();
    let files = signal_project::export::export_slot(&project, &args.slot, part.as_deref())
        .map_err(Failure::Usage)?;
    let samples = signal_project::export::sample_files(
        &args.project,
        files.iter().map(|f| (f.path.as_str(), f.text.as_str())),
    )
    .map_err(Failure::Usage)?;
    if args.out.exists()
        && std::fs::read_dir(&args.out)
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        return Err(Failure::Usage(format!(
            "{} exists and is not empty",
            args.out.display()
        )));
    }
    let texts = files.iter().map(|f| (f.path.as_str(), f.text.as_bytes()));
    let bytes = samples.iter().map(|(p, b)| (p.as_str(), &b[..]));
    let mut written: Vec<&str> = Vec::new();
    for (relative, content) in texts.chain(bytes) {
        let path = args.out.join(relative);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Failure::Io(format!("{}: {e}", dir.display())))?;
        }
        std::fs::write(&path, content)
            .map_err(|e| Failure::Io(format!("{}: {e}", path.display())))?;
        written.push(relative);
    }
    if json {
        print_stdout(
            &json!({"project": args.out.display().to_string(), "written": written}).to_string(),
        );
    } else {
        for path in written {
            print_stdout(&format!("wrote {path}"));
        }
    }
    Ok(())
}
