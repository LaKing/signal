//! `signal render`: load, validate, plan and render a project with the offline
//! driver, writing `audio.wav`, `manifest.json`, `log.jsonl` and `metrics.json`.

use std::path::{Path, PathBuf};

use clap::{ArgGroup, Args};
use serde_json::json;
use signal_core::{Registry, SinkKind};
use signal_engine::{Engine, EngineConfig, RenderError, render};
use signal_project::{Diagnostic, LoadError, Project};

use crate::provenance;
use crate::{Failure, print_stdout};

/// The files a render writes, in the order listed in output.
const FILES: [&str; 4] = ["audio.wav", "manifest.json", "log.jsonl", "metrics.json"];

/// Arguments of `signal render`.
#[derive(Args)]
#[command(group(ArgGroup::new("length").required(true).args(["seconds", "frames"])))]
pub(crate) struct RenderArgs {
    /// The project folder (containing project.toml).
    project: PathBuf,
    /// Render this many seconds (rounded to the nearest frame).
    #[arg(long)]
    seconds: Option<f64>,
    /// Render this many frames.
    #[arg(long)]
    frames: Option<u64>,
    /// The folder to write the artifacts to (created if needed).
    #[arg(long)]
    out: PathBuf,
    /// The master seed, instead of the project's.
    #[arg(long)]
    seed: Option<u64>,
    /// The sample rate in Hz, instead of the project's.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    sample_rate: Option<u32>,
}

/// Diagnostics with their file paths joined to the project folder.
fn located(dir: &Path, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .map(|d| Diagnostic {
            file: dir.join(&d.file).display().to_string(),
            ..d
        })
        .collect()
}

pub(crate) fn run(args: &RenderArgs, registry: &Registry, json: bool) -> Result<(), Failure> {
    let dir = &args.project;
    let project = Project::load(dir).map_err(|error| match error {
        LoadError::Invalid(diagnostics) => Failure::Invalid(located(dir, diagnostics)),
        error @ LoadError::Io { .. } => Failure::Io(error.to_string()),
    })?;
    let planned = project
        .plan(registry)
        .map_err(|diagnostics| Failure::Invalid(located(dir, diagnostics)))?;
    let plan = &planned.plan;

    let sinks: Vec<_> = plan
        .instances
        .iter()
        .filter(|i| matches!(i.descriptor.sink, Some(SinkKind::Audio)))
        .collect();
    match sinks.as_slice() {
        [_] => {}
        [] => {
            let message = "the project has no audio sink (an instance of `std.audio_out`)";
            let diagnostic = project.instances_error(message.to_owned());
            return Err(Failure::Invalid(located(dir, vec![diagnostic])));
        }
        several => {
            let diagnostics = several
                .iter()
                .map(|sink| {
                    let message = format!(
                        "{}: the project has {} audio sinks; a render needs exactly one",
                        sink.address,
                        several.len()
                    );
                    project.instance_error(&sink.address, message)
                })
                .collect();
            return Err(Failure::Invalid(located(dir, diagnostics)));
        }
    }

    let sample_rate = args.sample_rate.unwrap_or(project.sample_rate);
    let seed = args.seed.unwrap_or(project.seed);
    let frames = match (args.frames, args.seconds) {
        (Some(frames), _) => frames,
        (None, Some(seconds)) => frames_of(seconds, sample_rate)?,
        (None, None) => return Err(Failure::Usage("give --seconds or --frames".to_owned())),
    };

    let config = EngineConfig::new(sample_rate, seed);
    let mut engine = Engine::new(plan, registry, config, Vec::new())
        .map_err(|e| Failure::Internal(e.to_string()))?;
    let rendered = render(&mut engine, frames).map_err(|e| render_failure(&project, dir, e))?;
    let provenance = provenance::collect(&provenance::source_dir(), &project.files);
    let manifest = rendered
        .write(&args.out, provenance)
        .map_err(|e| render_failure(&project, dir, e))?;
    let samples: Vec<f32> = rendered.samples.iter().map(|&x| x as f32).collect();
    let metrics = signal_eval::metrics(&samples, rendered.channels);
    let metrics_path = args.out.join("metrics.json");
    metrics
        .write(&metrics_path)
        .map_err(|e| Failure::Io(format!("writing {}: {e}", metrics_path.display())))?;

    let warnings = located(dir, planned.warnings.clone());
    if json {
        let output = json!({
            "out": args.out.display().to_string(),
            "frames": rendered.frames,
            "sample_rate": rendered.sample_rate,
            "channels": rendered.channels,
            "files": FILES,
            "warnings": warnings,
            "manifest": manifest,
            "metrics": metrics,
        });
        print_stdout(&serde_json::to_string_pretty(&output).unwrap_or_default());
    } else {
        for warning in &warnings {
            eprintln!("{warning}");
        }
        print_stdout(&format!(
            "rendered {} frames at {} Hz ({} channels) to {}\naudio_fnv1a64 {}",
            rendered.frames,
            rendered.sample_rate,
            rendered.channels,
            args.out.display(),
            manifest.audio_fnv1a64
        ));
    }
    Ok(())
}

/// The frame count of `seconds` at `sample_rate`, rounded half up.
fn frames_of(seconds: f64, sample_rate: u32) -> Result<u64, Failure> {
    let frames = (seconds * f64::from(sample_rate) + 0.5).floor();
    if !seconds.is_finite() || seconds < 0.0 || frames >= u64::MAX as f64 {
        return Err(Failure::Usage(format!(
            "--seconds must be a finite, non-negative number of seconds, not {seconds}"
        )));
    }
    Ok(frames as u64)
}

fn render_failure(project: &Project, dir: &Path, error: RenderError) -> Failure {
    match error {
        RenderError::Io(_) | RenderError::Wav(hound::Error::IoError(_)) => {
            Failure::Io(error.to_string())
        }
        RenderError::AudioChannels { ref instance, .. } => {
            let diagnostic = project.instance_error(instance, error.to_string());
            Failure::Invalid(located(dir, vec![diagnostic]))
        }
        error => Failure::Internal(error.to_string()),
    }
}
