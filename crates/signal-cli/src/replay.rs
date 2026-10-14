//! `signal replay`: load and validate a session directory
//! (`docs/session-format.md`), replay it offline through the engine's live
//! edit API, exactly as it was played or (`--ideal`) with every latent
//! result awaited, and write the same artifacts as `signal render`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::json;
use signal_core::Registry;
use signal_project::LoadError;
use signal_project::provenance;
use signal_project::session::{
    BoundaryReplay, Here, Machine, Replay, ReplayError, ReplayOptions, Session, replay_with,
};

use crate::gpu::Gpu;

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
    /// Threads a block runs on, the calling thread included (0: one per
    /// performance core; 1: no worker threads). The output is identical for
    /// every count.
    #[arg(long, default_value_t = 0)]
    threads: u32,
    /// Render ideally: wait for every result of a latent (GPU-backed)
    /// module, whatever the session recorded. Not what was played: the
    /// manifest says `ideal`.
    #[arg(long)]
    ideal: bool,
    /// Replay exactly although the session was recorded by another build or
    /// on another machine, whose results of latent modules this one must
    /// recompute. Each result is still checked against its recorded hash;
    /// the manifest says `trust_provenance`.
    #[arg(long, conflicts_with = "ideal")]
    trust_provenance: bool,
}

pub(crate) fn run(
    args: &ReplayArgs,
    registry: &Registry,
    setup: Option<&Path>,
    json: bool,
) -> Result<(), Failure> {
    let dir = &args.session;
    let session = Session::load(dir, registry).map_err(|error| match error {
        LoadError::Invalid(diagnostics) => Failure::InvalidSession(located(dir, diagnostics)),
        error @ LoadError::Io { .. } => Failure::Io(error.to_string()),
    })?;
    let provenance = provenance::collect(
        &provenance::source_dir(),
        session.versions.first().map_or(&[][..], |p| &p.files),
    );
    // M2.7-2: latent instances placed on this machine's devices, by the
    // first version's pins (a running project cannot change them).
    let pins = match (session.versions.first(), session.plans.first()) {
        (Some(project), Some(plan)) => project.gpu_pins(plan),
        _ => BTreeMap::new(),
    };
    let mut gpu = Gpu::new(registry, session.latent(), pins, setup);
    // The build and machine an exact replay compares with the recording's
    // (M2.7-8), with the GPU devices its latent modules will use.
    let here = session.latent().then(|| Here {
        build: provenance.clone(),
        machine: Machine::here(gpu.devices()),
    });
    let options = ReplayOptions {
        frames: args.frames,
        threads: args.threads as usize,
        ideal: args.ideal,
        trust_provenance: args.trust_provenance,
        here,
    };
    let mut replayed = replay_with(&session, gpu.registry(), options).map_err(|e| match e {
        ReplayError::IncompleteBoundaries { .. }
        | ReplayError::Unreproduced { .. }
        | ReplayError::BeyondRecordedEnd { .. }
        | ReplayError::ProvenanceMismatch { .. } => Failure::Replay(e.to_string()),
        ReplayError::ReplayMismatch { .. } => Failure::Mismatch(e.to_string()),
        e => Failure::Internal(e.to_string()),
    })?;
    (replayed.render.compute_devices, replayed.render.placement) = gpu.record();
    let rendered = &replayed.render;

    let mut warnings = gpu.warnings();
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
    warnings.extend(boundary_warnings(&replayed, session.latent()));
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
            "ideal": replayed.boundaries.ideal(),
            "unverified": replayed.unverified,
            "trust_provenance": replayed.trusted_provenance,
            "boundaries": match replayed.boundaries {
                BoundaryReplay::NotLatent => "not-latent",
                BoundaryReplay::Exact => "exact",
                BoundaryReplay::NotRecorded => "not-recorded",
                BoundaryReplay::Ideal => "ideal",
            },
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

/// What a replay of a session (with latent modules, `latent`) says about its
/// boundary results beside its output.
fn boundary_warnings(replayed: &Replay, latent: bool) -> Vec<String> {
    let mut warnings = Vec::new();
    match replayed.boundaries {
        BoundaryReplay::NotRecorded => warnings.push(
            "the session records no boundary outcomes (it predates them): misses of latent modules were not recorded, so this replay waits for every result, an ideal render".to_owned(),
        ),
        BoundaryReplay::Ideal if latent => warnings.push(
            "an ideal render: every result of a latent module was awaited; this is not what was played".to_owned(),
        ),
        _ => {}
    }
    // M2.7-8: a session recorded before result hashes replays as before,
    // but nothing shows its recomputed results are the performance's.
    if replayed.unverified > 0 {
        warnings.push(format!(
            "exactness is unverified: the session predates result hashes, so {} recorded results of latent modules could not be checked against the performance; `signal replay --ideal` renders with every result awaited, and a session recorded with `signal play --record-boundary-data` replays exactly anywhere",
            replayed.unverified
        ));
    }
    if replayed.trusted_provenance {
        warnings.push(
            "--trust-provenance: the session was recorded by another build or on another machine; its recomputed results matched their recorded hashes, and the manifest says trust_provenance".to_owned(),
        );
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C8: a replay of a session written before result hashes warns that
    /// its exactness is unverified, naming the ways to an exact or an ideal
    /// render; a verified exact replay warns of nothing.
    #[test]
    fn an_unverified_exact_replay_warns() {
        let mut replayed = Replay {
            render: signal_output::Render {
                sample_rate: 1,
                seed: 0,
                frames: 0,
                channels: 1,
                samples: Vec::new(),
                log: Vec::new(),
                same_machine: Vec::new(),
                conversion: signal_output::Conversion::Device,
                ideal: false,
                trust_provenance: false,
                compute_devices: Vec::new(),
                placement: Vec::new(),
            },
            skipped: 0,
            boundaries: BoundaryReplay::Exact,
            unverified: 0,
            trusted_provenance: false,
        };
        assert!(boundary_warnings(&replayed, true).is_empty());
        replayed.unverified = 3;
        let warnings = boundary_warnings(&replayed, true);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let warning = &warnings[0];
        assert!(
            warning.contains("unverified")
                && warning.contains("3 recorded results")
                && warning.contains("--ideal")
                && warning.contains("--record-boundary-data"),
            "{warning}"
        );
    }
}
