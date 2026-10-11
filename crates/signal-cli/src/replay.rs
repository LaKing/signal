//! `signal replay`: load and validate a session directory
//! (`docs/session-format.md`), replay it offline through the engine's live
//! edit API, and write the same artifacts as `signal render`.

use std::path::PathBuf;

use clap::Args;
use serde_json::json;
use signal_cli::session::{Session, replay};
use signal_core::Registry;
use signal_project::LoadError;

use crate::provenance;
use crate::render::{FILES, located, metrics_of, write_artifacts};
use crate::{Failure, print_stdout};

/// Arguments of `signal replay`.
#[derive(Args)]
pub(crate) struct ReplayArgs {
    /// The session directory (containing session.toml).
    session: PathBuf,
    /// The folder to write the artifacts to (created if needed). Without it,
    /// nothing is written and only the hashes are printed.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Render this many frames, instead of the last command's frame plus one
    /// second.
    #[arg(long)]
    frames: Option<u64>,
}

pub(crate) fn run(args: &ReplayArgs, registry: &Registry, json: bool) -> Result<(), Failure> {
    let dir = &args.session;
    let session = Session::load(dir, registry).map_err(|error| match error {
        LoadError::Invalid(diagnostics) => Failure::InvalidSession(located(dir, diagnostics)),
        error @ LoadError::Io { .. } => Failure::Io(error.to_string()),
    })?;
    let replayed =
        replay(&session, registry, args.frames).map_err(|e| Failure::Internal(e.to_string()))?;
    let rendered = &replayed.render;

    let provenance = provenance::collect(
        &provenance::source_dir(),
        session.versions.first().map_or(&[][..], |p| &p.files),
    );
    let mut warnings = Vec::new();
    let recorded = &session.provenance;
    if recorded.signal_revision != provenance.signal_revision
        || recorded.rustc_version != provenance.rustc_version
        || recorded.cargo_lock_hash != provenance.cargo_lock_hash
    {
        warnings.push(format!(
            "the session was recorded by another build (revision {}, {}, Cargo.lock {}); replay is bit-exact only on the same build (ADR 0008)",
            recorded.signal_revision, recorded.rustc_version, recorded.cargo_lock_hash
        ));
    }
    if replayed.skipped > 0 {
        warnings.push(format!(
            "{} commands at or after frame {} were not applied",
            replayed.skipped, rendered.frames
        ));
    }

    let (manifest, metrics) = match &args.out {
        Some(out) => write_artifacts(rendered, out, provenance)
            .map_err(|e| e.into_failure(|e| Failure::Io(e.to_string())))?,
        None => (rendered.manifest(provenance), metrics_of(rendered)),
    };

    if json {
        let output = json!({
            "session": dir.display().to_string(),
            "out": args.out.as_ref().map(|out| out.display().to_string()),
            "frames": rendered.frames,
            "sample_rate": rendered.sample_rate,
            "channels": rendered.channels,
            "commands": session.edits.len(),
            "skipped": replayed.skipped,
            "files": if args.out.is_some() { &FILES[..] } else { &[][..] },
            "warnings": warnings,
            "manifest": manifest,
            "metrics": metrics,
        });
        print_stdout(&serde_json::to_string_pretty(&output).unwrap_or_default());
    } else {
        for warning in &warnings {
            eprintln!("signal: warning: {warning}");
        }
        let target = args
            .out
            .as_ref()
            .map(|out| format!(" to {}", out.display()))
            .unwrap_or_default();
        print_stdout(&format!(
            "replayed {} commands over {} frames at {} Hz ({} channels){target}\naudio_fnv1a64 {}\nlog_fnv1a64 {}",
            session.edits.len() - replayed.skipped,
            rendered.frames,
            rendered.sample_rate,
            rendered.channels,
            manifest.audio_fnv1a64,
            manifest.log_fnv1a64
        ));
    }
    Ok(())
}
