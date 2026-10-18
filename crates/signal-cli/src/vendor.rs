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
    /// The library composite, `lib.<name>` (`signal modules` lists them).
    composite: String,
    /// The name of the copy (default: its name without `lib.`). The library
    /// composites it uses keep their names without `lib.`.
    #[arg(long = "as")]
    name: Option<String>,
}

/// `signal vendor`.
pub(crate) fn run(args: &VendorArgs, json: bool) -> Result<(), Failure> {
    let vendored = vendor::vendor(
        &args.project,
        &args.composite,
        args.name.as_deref(),
        Library::builtin(),
    )
    .map_err(|error| match error {
        VendorError::Io { .. } => Failure::Io(error.to_string()),
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
