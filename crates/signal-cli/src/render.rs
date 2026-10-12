//! `signal render`: load, validate, plan and render a project with the offline
//! driver, writing `audio.wav`, `manifest.json`, `log.jsonl` and `metrics.json`.

use std::path::{Path, PathBuf};

use clap::{ArgGroup, Args};
use serde_json::json;
use signal_core::{Registry, SinkKind};
use signal_engine::{Engine, EngineConfig, Manifest, Provenance, Render, RenderError, render};
use signal_eval::Metrics;
pub(crate) use signal_project::located;
use signal_project::provenance;
use signal_project::{LoadError, Project};

use crate::{Failure, print_stdout};

/// The files a render writes, in the order listed in output.
pub(crate) const FILES: [&str; 4] = ["audio.wav", "manifest.json", "log.jsonl", "metrics.json"];

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
    /// Frames the engine runs at once (1 to 4096). A performance setting:
    /// the output is identical for every block size.
    #[arg(long, default_value_t = 256, value_parser = clap::value_parser!(u32).range(1..=4096))]
    block: u32,
    /// Threads a block runs on, the calling thread included (0: one per
    /// performance core; 1: no worker threads). A performance setting: the
    /// output is identical for every count.
    #[arg(long, default_value_t = 0)]
    threads: u32,
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

    let config = EngineConfig {
        block_capacity: args.block as usize,
        threads: args.threads as usize,
        ..EngineConfig::new(sample_rate, seed)
    };
    let mut engine = Engine::new(plan, registry, config, Vec::new())
        .map_err(|e| Failure::Internal(e.to_string()))?;
    let rendered = render(&mut engine, frames).map_err(|e| render_failure(&project, dir, e))?;
    let provenance = provenance::collect(&provenance::source_dir(), &project.files);
    let (manifest, metrics) = write_artifacts(&rendered, &args.out, provenance)
        .map_err(|e| e.into_failure(|e| render_failure(&project, dir, e)))?;

    let warnings = located(dir, planned.warnings.clone());
    // ADR 0019 §3: offline the engine waits for a due result, so any late
    // result was lost (no usable GPU): the render is not what the project
    // means. Said only when it happened, so other renders print as before.
    let late = engine.late_results();
    if json {
        let mut output = json!({
            "out": args.out.display().to_string(),
            "frames": rendered.frames,
            "sample_rate": rendered.sample_rate,
            "channels": rendered.channels,
            "files": FILES,
            "warnings": warnings,
            "manifest": manifest,
            "metrics": metrics,
        });
        if late > 0 {
            output["late_results"] = json!(late);
        }
        print_stdout(&serde_json::to_string_pretty(&output).unwrap_or_default());
    } else {
        for warning in &warnings {
            eprintln!("{warning}");
        }
        if late > 0 {
            eprintln!(
                "warning: {late} due results of GPU-backed modules were lost (late_result faults in log.jsonl): those blocks are silent"
            );
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

/// Why [`write_artifacts`] failed.
pub(crate) enum WriteError {
    /// Writing `audio.wav`, `log.jsonl` or `manifest.json`.
    Render(RenderError),
    /// Writing `metrics.json`.
    Metrics(String),
}

impl WriteError {
    pub(crate) fn into_failure(self, render: impl FnOnce(RenderError) -> Failure) -> Failure {
        match self {
            WriteError::Render(error) => render(error),
            WriteError::Metrics(message) => Failure::Io(message),
        }
    }
}

/// Writes the render artifacts of `rendered` into `out`: `audio.wav`,
/// `log.jsonl`, `manifest.json` and `metrics.json` (shared by `signal render`
/// and `signal replay`).
pub(crate) fn write_artifacts(
    rendered: &Render,
    out: &Path,
    provenance: Provenance,
) -> Result<(Manifest, Metrics), WriteError> {
    let manifest = rendered
        .write(out, provenance)
        .map_err(WriteError::Render)?;
    let metrics = metrics_of(rendered);
    let metrics_path = out.join("metrics.json");
    metrics
        .write(&metrics_path)
        .map_err(|e| WriteError::Metrics(format!("writing {}: {e}", metrics_path.display())))?;
    Ok((manifest, metrics))
}

/// The metrics of a render's audio, as `metrics.json` holds them.
pub(crate) fn metrics_of(rendered: &Render) -> Metrics {
    let samples: Vec<f32> = rendered.samples.iter().map(|&x| x as f32).collect();
    signal_eval::metrics(&samples, rendered.channels)
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
