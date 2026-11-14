//! `signal vendor`: a library composite copied into a project (ADR 0027 §4;
//! the work is `signal_project::vendor`).

use std::path::PathBuf;

use clap::Args;
use serde_json::json;
use signal_project::Library;
use signal_project::vendor::{self, VendorError};

use crate::{Failure, print_stdout};

/// Arguments of `signal vendor`.
#[derive(Args)]
pub(crate) struct VendorArgs {
    /// The project folder.
    project: PathBuf,
    /// The library composite, `lib.<name>` (`signal modules` lists them);
    /// none with `--slot` or `--include`.
    #[arg(required_unless_present_any = ["slot", "include"])]
    composite: Option<String>,
    /// ADR 0037 phase 4, ADR 0051 §10: instead, make top-level slot SLOT,
    /// which links a project (`load = "<path>"`), hold its own copy of it
    /// (its inside, composites, patterns and panel as the slot's part).
    #[arg(long, conflicts_with_all = ["composite", "name", "include"])]
    slot: Option<String>,
    /// ADR 0035 §11: instead, turn the top-level include INCLUDE (`project =
    /// "<path>"`) into the project's own composite (named by `--as`, default
    /// INCLUDE) with its seed, so the project renders the same.
    #[arg(long, conflicts_with = "composite")]
    include: Option<String>,
    /// The name of the copy (default: its name without `lib.`). The library
    /// composites it uses keep their names without `lib.`.
    #[arg(long = "as")]
    name: Option<String>,
}

/// `signal vendor`.
pub(crate) fn run(args: &VendorArgs, json: bool) -> Result<(), Failure> {
    let vendored = match (&args.slot, &args.composite) {
        (Some(slot), _) => vendor::prepare_slot(&args.project, slot, Library::builtin())
            .and_then(|prepared| vendor::write(&args.project, prepared, Library::builtin())),
        (None, _) if args.include.is_some() => vendor::prepare_include(
            &args.project,
            args.include.as_deref().unwrap_or_default(),
            args.name.as_deref(),
            Library::builtin(),
        )
        .and_then(|prepared| vendor::write(&args.project, prepared, Library::builtin())),
        (None, Some(composite)) => vendor::vendor(
            &args.project,
            composite,
            args.name.as_deref(),
            Library::builtin(),
        ),
        (None, None) => {
            return Err(Failure::Usage(
                "name a library composite, or a slot with --slot".to_owned(),
            ));
        }
    }
    .map_err(|error| match error {
        // A conflict is a runtime failure of the files, not of the arguments.
        VendorError::Io { .. } | VendorError::Changed(_) => Failure::Io(error.to_string()),
        error => Failure::Usage(error.to_string()),
    })?;
    if json {
        print_stdout(
            &json!({
                "project": args.project.display().to_string(),
                "copied": vendored.copied,
                "reused": vendored.reused,
                "rewritten": vendored.rewritten,
            })
            .to_string(),
        );
    } else {
        for path in &vendored.copied {
            print_stdout(&format!("copied {path}"));
        }
        for path in &vendored.reused {
            print_stdout(&format!("kept {path} (already vendored, identical)"));
        }
        for path in &vendored.rewritten {
            print_stdout(&format!("rewrote the references in {path}"));
        }
    }
    Ok(())
}
