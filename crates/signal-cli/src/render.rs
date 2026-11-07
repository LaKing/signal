//! `signal render`: load, validate, plan and render a project with the offline
//! driver, writing `audio.wav`, `manifest.json`, `log.jsonl` and `metrics.json`.

use std::path::{Path, PathBuf};

use crate::gpu::{Gpu, uses_gpu};

use clap::{ArgGroup, Args};
use serde_json::{Value, json};
use signal_core::Registry;
use signal_engine::{Engine, EngineConfig, Manifest, Provenance};
use signal_eval::Metrics;
use signal_output::inputs::{RecordedInput, RunInput};
use signal_output::{Render, RenderError, render_fed};
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
    /// The project folder (containing project.toml), or a .signal file.
    pub(crate) project: PathBuf,
    /// Render this many seconds (rounded to the nearest frame).
    #[arg(long)]
    pub(crate) seconds: Option<f64>,
    /// Render this many frames.
    #[arg(long)]
    pub(crate) frames: Option<u64>,
    /// The folder to write the artifacts to (created if needed).
    #[arg(long)]
    pub(crate) out: PathBuf,
    /// The master seed, instead of the project's.
    #[arg(long)]
    pub(crate) seed: Option<u64>,
    /// The sample rate in Hz, instead of the project's.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) sample_rate: Option<u32>,
    /// Frames the engine runs at once (1 to 4096). A performance setting:
    /// the output is identical for every block size.
    #[arg(long, default_value_t = 256, value_parser = clap::value_parser!(u32).range(1..=4096))]
    pub(crate) block: u32,
    /// Threads a block runs on, the calling thread included (0: one per
    /// performance core; 1: no worker threads). A performance setting: the
    /// output is identical for every count.
    #[arg(long, default_value_t = 0)]
    pub(crate) threads: u32,
    /// Feeds capture slot SLOT from a WAV file, for this render only (ADR
    /// 0038 §9): `--input mic=take.wav`. Its rate must be the render's and
    /// its channels the slot's; past its end the slot reads 0, as a slot
    /// with none does. The manifest records its hash. Repeatable.
    #[arg(long = "input", value_name = "SLOT=WAV")]
    pub(crate) inputs: Vec<String>,
}

pub(crate) fn run(
    args: &RenderArgs,
    registry: &Registry,
    setup: Option<&Path>,
    json: bool,
) -> Result<(), Failure> {
    let rendered = render_project(args, registry, setup)?;
    if json {
        print_stdout(&serde_json::to_string_pretty(&rendered.output).unwrap_or_default());
    } else {
        for note in &rendered.notes {
            eprintln!("{note}");
        }
        print_stdout(&rendered.text);
    }
    Ok(())
}

/// What a render wrote and says: `--json`'s object, the text summary, and
/// the warnings it gives on stderr (shared by `signal render` and `signal
/// mcp`'s `render` tool).
pub(crate) struct Rendered {
    pub(crate) output: Value,
    pub(crate) text: String,
    pub(crate) notes: Vec<String>,
}

/// Renders the project `args` names into `args.out`.
pub(crate) fn render_project(
    args: &RenderArgs,
    registry: &Registry,
    setup: Option<&Path>,
) -> Result<Rendered, Failure> {
    // A .signal file is read from a temporary folder, kept until the end.
    let opened = crate::files::project_arg(&args.project)?;
    let (load_dir, dir) = (opened.dir(), opened.shown());
    let project = Project::load(load_dir).map_err(|error| match error {
        LoadError::Invalid(diagnostics) => Failure::Invalid(located(dir, diagnostics)),
        error @ LoadError::Io { .. } => Failure::Io(error.to_string()),
    })?;
    let planned = project
        .plan(registry)
        .map_err(|diagnostics| Failure::Invalid(located(dir, diagnostics)))?;
    let plan = &planned.plan;

    // ADR 0038 §9: `audio.wav` records the main play side; each other
    // (a device slot of another role) is written as `devices/<slot>.wav`.
    // The plan refuses two sinks of one role.
    let sides = signal_project::device::play_sides(&project, plan);
    let Some(main) = signal_project::device::main_side(&sides).map(|s| s.address.clone()) else {
        let message = "the project has no audio sink (an instance of `std.audio_out`)";
        let diagnostic = project.instances_error(message.to_owned());
        return Err(Failure::Invalid(located(dir, vec![diagnostic])));
    };
    let mut others: Vec<_> = sides
        .iter()
        .filter(|s| s.address != main)
        .map(|s| s.address.clone())
        .collect();
    others.sort();

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
    // M2.7-2: GPU-backed instances on this machine's devices; dropped after
    // the engine.
    let mut gpu = Gpu::new(registry, uses_gpu(plan), project.gpu_pins(plan), setup);
    // ADR 0038 §9: the capture slots `--input` feeds; the others read 0.
    let (mut inputs, input_hashes) =
        render_inputs(&args.inputs, plan, sample_rate, args.block as usize)?;
    // ADR 0038 §3, §9: a capture slot's own `render` file feeds it unless
    // `--input` does; the file is a project file, hashed as one.
    let mut fixture_hashes = std::collections::BTreeMap::new();
    for (name, slot) in project.device_slots() {
        let Some(file) = &slot.render else {
            continue;
        };
        if input_hashes.contains_key(name) {
            continue;
        }
        let address: signal_core::InstanceAddress = format!("/{name}")
            .parse()
            .map_err(|_| Failure::Internal(format!("`{name}` is no instance address")))?;
        let Some((_, width)) = signal_project::session::plan_sources(plan)
            .into_iter()
            .find(|(a, _)| *a == address)
        else {
            continue;
        };
        let invalid = |message: String| {
            Failure::Invalid(located(dir, vec![project.instances_error(message)]))
        };
        let path = signal_project::render_file(load_dir, file)
            .map_err(|e| invalid(format!("{name}: render file `{file}`: {e}")))?;
        let bytes = std::fs::read(&path)
            .map_err(|e| invalid(format!("{name}: render file `{file}`: {e}")))?;
        let words = read_wav(&path, sample_rate, width)
            .map_err(|e| invalid(format!("{name}: render file `{file}`: {e}")))?;
        fixture_hashes.insert(
            file.clone(),
            format!("{:016x}", signal_core::fnv1a64(&bytes)),
        );
        let (input, _) = RunInput::new(
            address,
            width,
            Box::new(RecordedInput::new(words, width)),
            args.block as usize,
            None,
        );
        inputs.push(input);
    }
    let mut engine = Engine::new(plan, gpu.registry(), config, Vec::new())
        .map_err(|e| Failure::Internal(e.to_string()))?;
    let sides = (!others.is_empty()).then_some((&main, &others[..]));
    let (mut rendered, side_renders) = render_fed(&mut engine, frames, sides, inputs)
        .map_err(|e| render_failure(&project, dir, e))?;
    (rendered.compute_devices, rendered.placement) = gpu.record();
    let gpu_warnings = gpu.warnings();
    let mut provenance = provenance::collect(&provenance::source_dir(), &project.files);
    provenance.library_composite_hashes = project.library_hashes();
    // ADR 0035 §13, ADR 0037 §3: what includes and slots bring; absent
    // without them.
    provenance
        .project_file_hashes
        .extend(project.include_file_hashes());
    provenance.included_projects = project.included_projects();
    provenance.library_interface_hashes = project.library_interface_hashes();
    provenance.render_input_hashes = input_hashes;
    provenance.project_file_hashes.extend(fixture_hashes);
    let (manifest, metrics) = write_artifacts(&rendered, &args.out, provenance)
        .map_err(|e| e.into_failure(|e| render_failure(&project, dir, e)))?;
    // ADR 0038 §9: the other play sides, each in its own WAV.
    let mut devices = serde_json::Map::new();
    for side in &side_renders {
        let name = side.address.as_str().trim_start_matches('/');
        let file = format!("devices/{name}.wav");
        side.write(&args.out.join(&file), rendered.sample_rate)
            .map_err(|e| render_failure(&project, dir, e))?;
        devices.insert(
            name.to_owned(),
            json!({"file": file, "channels": side.channels, "audio_fnv1a64": format!("{:016x}", side.audio_fnv1a64())}),
        );
    }

    let warnings = located(dir, planned.warnings.clone());
    // ADR 0019 §3: offline the engine waits for a due result, so any late
    // result was lost (no usable GPU): the render is not what the project
    // means. Said only when it happened, so other renders print as before.
    let late = engine.late_results();
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
    // ADR 0035 §8: an include written for another rate plays at the
    // render's; said, never an error.
    let rate_notes = project.rate_notes(sample_rate);
    if !rate_notes.is_empty() {
        output["notes"] = json!(rate_notes);
    }
    if late > 0 {
        output["late_results"] = json!(late);
    }
    if !devices.is_empty() {
        output["devices"] = serde_json::Value::Object(devices);
    }
    if !gpu_warnings.is_empty() {
        output["gpu_warnings"] = json!(gpu_warnings);
    }
    if let Some(setup) = gpu.setup_json() {
        output["gpu_setup"] = setup;
    }
    let mut notes: Vec<String> = warnings.iter().map(ToString::to_string).collect();
    notes.extend(gpu_warnings.iter().map(|w| format!("signal: {w}")));
    notes.extend(rate_notes.iter().map(|n| format!("note: {n}")));
    if late > 0 {
        notes.push(format!(
            "warning: {late} due results of GPU-backed modules were lost (late_result faults in log.jsonl): those blocks are silent"
        ));
    }
    let text = format!(
        "rendered {} frames at {} Hz ({} channels) to {}\naudio_fnv1a64 {}",
        rendered.frames,
        rendered.sample_rate,
        rendered.channels,
        args.out.display(),
        manifest.audio_fnv1a64
    );
    Ok(Rendered {
        output,
        text,
        notes,
    })
}

/// The capture slots `specs` (`SLOT=WAV`) feed: one [`RunInput`] each, the
/// WAV's samples as f32, and the FNV-1a 64 of those words by slot. A slot
/// is a source of `plan` by its name (`mic`, `band.mic`) or address; its
/// WAV must be at `rate` with the slot's channels.
fn render_inputs(
    specs: &[String],
    plan: &signal_graph::ExecutionPlan,
    rate: u32,
    block: usize,
) -> Result<(Vec<RunInput>, std::collections::BTreeMap<String, String>), Failure> {
    let sources = signal_project::session::plan_sources(plan);
    let mut inputs = Vec::new();
    let mut hashes = std::collections::BTreeMap::new();
    for spec in specs {
        let (slot, file) = spec
            .split_once('=')
            .filter(|(slot, file)| !slot.is_empty() && !file.is_empty())
            .ok_or_else(|| Failure::Usage(format!("--input {spec}: give SLOT=WAV")))?;
        let found = sources.iter().find(|(address, _)| {
            signal_project::session::slot_name(address) == slot || address.to_string() == slot
        });
        let Some((address, width)) = found else {
            let names: Vec<String> = sources
                .iter()
                .map(|(a, _)| signal_project::session::slot_name(a))
                .collect();
            return Err(Failure::Usage(format!(
                "--input {spec}: `{slot}` is no capture slot of the project{}",
                if names.is_empty() {
                    " (it has none)".to_owned()
                } else {
                    format!(" ({})", names.join(", "))
                }
            )));
        };
        let slot = signal_project::session::slot_name(address);
        if hashes.contains_key(&slot) {
            return Err(Failure::Usage(format!("--input: `{slot}` is fed twice")));
        }
        let words = read_wav(Path::new(file), rate, *width)
            .map_err(|e| Failure::Usage(format!("--input {spec}: {e}")))?;
        hashes.insert(slot, format!("{:016x}", signal_core::fnv1a64_words(&words)));
        let (input, _) = RunInput::new(
            address.clone(),
            *width,
            Box::new(RecordedInput::new(words, *width)),
            block,
            None,
        );
        inputs.push(input);
    }
    Ok((inputs, hashes))
}

/// The samples of WAV `path` as f32, interleaved: float WAVs as stored,
/// integer ones scaled to ±1. Its rate must be `rate` and its channels
/// `channels`.
fn read_wav(path: &Path, rate: u32, channels: usize) -> Result<Vec<f32>, String> {
    let mut reader =
        hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = reader.spec();
    if spec.sample_rate != rate {
        return Err(format!(
            "{} is at {} Hz; the render runs at {rate} Hz",
            path.display(),
            spec.sample_rate
        ));
    }
    if usize::from(spec.channels) != channels {
        return Err(format!(
            "{} has {} channels; the slot has {channels}",
            path.display(),
            spec.channels
        ));
    }
    let read = |e: hound::Error| format!("{}: {e}", path.display());
    match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<f32>, _>>()
            .map_err(read),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.clamp(1, 32) - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|s| s as f32 * scale))
                .collect::<Result<Vec<f32>, _>>()
                .map_err(read)
        }
    }
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
    let samples: Vec<f32> = rendered
        .samples
        .iter()
        .map(|&x| rendered.conversion.sample(x))
        .collect();
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
