//! `signal ui`: plays a project as `signal play --watch` does, with the same
//! driver options, and serves it to web pages through the UI protocol
//! (`signal-ui`, `docs/ui-protocol.md`): the UI files over HTTP and a
//! WebSocket, on 127.0.0.1 unless `--remote`. `--open` opens the page in the
//! default browser. This thread is the control thread: it steps the host.
//!
//! `signal ui check <project>` checks the project's panels (`ui/*.html`)
//! against its plan instead: every `bind` names a port whose kind and shape
//! fit its control (`signal_ui::bindings`).

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use clap::{Args, Subcommand};
use serde_json::json;
use signal_core::Registry;
use signal_engine::{ControlSink, RtOptions};
use signal_project::session::{SessionError, sink_of};
use signal_project::{LoadError, Project, provenance};
use signal_ui::bindings::{MAIN_PANEL, check_project};
use signal_ui::player::Backend;
use signal_ui::setup::setup_path;
use signal_ui::{FirstRun, Host, HostConfig, ServerOptions, project_ui};

use crate::devices::compute_lines;
use crate::drive::{Pace, drive};
use crate::files::{project_arg, temporary_note};
use crate::gpu::Gpu;
use crate::play::{DriverKind, PlayArgs, check, write_wav};
use crate::render::located;
use crate::{Failure, print_stdout};

/// Each client's queue of outgoing messages.
const CLIENT_QUEUE: usize = 256;

/// Arguments of `signal ui`.
#[derive(Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub(crate) struct UiArgs {
    #[command(subcommand)]
    command: Option<UiCommand>,
    #[command(flatten)]
    play: Option<PlayArgs>,
    /// Open the project's main panel in the default browser.
    #[arg(long)]
    open: bool,
    /// The HTTP port (0: a free one). The WebSocket takes a free port.
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Serve remote clients: bind every network interface instead of
    /// 127.0.0.1 (off by default). The session token then travels in clear
    /// text over the network.
    #[arg(long)]
    remote: bool,
    /// The UI library folder (default: the app bundle's, else the
    /// repository's `ui/`).
    #[arg(long)]
    ui_dir: Option<PathBuf>,
}

/// `signal ui`'s subcommands.
#[derive(Subcommand)]
enum UiCommand {
    /// Check the project's panels (ui/*.html): every bind must name a port
    /// of the plan whose kind and shape fit its control. Exits 1 with
    /// file:line:column diagnostics when one does not.
    Check {
        /// The project folder (containing project.toml), or a .signal file.
        project: PathBuf,
    },
}

pub(crate) fn run(
    args: &UiArgs,
    registry: &Registry,
    setup: Option<&Path>,
    json: bool,
) -> Result<(), Failure> {
    let play = match (&args.command, &args.play) {
        (Some(UiCommand::Check { project }), _) => return check_panels(project, registry, json),
        (None, Some(play)) => play,
        (None, None) => {
            return Err(Failure::Usage(
                "signal ui needs a project folder (or `signal ui check <project>`)".to_owned(),
            ));
        }
    };
    check(play)?;
    let null = play.driver == DriverKind::Null;
    let played = Played {
        project: &play.project,
        null,
        device: play.device.clone(),
        buffer: play.buffer,
        threads: play.threads,
        rt: RtOptions {
            policy: play.rt_policy,
            computation: play.rt_computation,
            trace: play.rt_trace,
        },
        boundary_data: play.record_boundary_data,
        recording: play.record_session.is_some() || play.record_audio.is_some(),
        ceiling: None,
    };
    let server = ServerOptions {
        port: args.port,
        remote: args.remote,
        ui_dir: args.ui_dir.clone().unwrap_or_else(default_ui_dir),
        project_ui: PathBuf::new(),
        client_queue: CLIENT_QUEUE,
    };
    let Serve {
        config,
        gpu,
        shown,
        width,
    } = serve(&played, registry, setup, server)?;
    let mut host = Host::new(config).map_err(Failure::Device)?;
    let rate = host.sample_rate();
    let url = host.url();
    let panel = host.panel_url();
    let schematic = host.page_url(signal_ui::SCHEMATIC_PAGE);
    eprintln!("signal: ui at {url}");
    eprintln!("signal: panel at {panel}");
    eprintln!("signal: schematic at {schematic}");
    if json {
        print_stdout(
            &json!({
                "url": url,
                "panel": panel,
                "schematic": schematic,
                "http": host.http_addr().to_string(),
                "ws": host.ws_addr().to_string(),
            })
            .to_string(),
        );
    }
    if args.remote {
        eprintln!(
            "signal: warning: serving remote clients on every interface; the token travels in clear text"
        );
    }
    eprintln!(
        "signal: playing {} at {rate} Hz{}; press Enter to stop",
        shown.display(),
        if null { " with the null driver" } else { "" }
    );
    if args.open {
        match std::process::Command::new("/usr/bin/open")
            .arg(&panel)
            .status()
        {
            Ok(status) if status.success() => {}
            Ok(status) => eprintln!("signal: warning: open {panel} failed ({status})"),
            Err(error) => eprintln!("signal: warning: open {panel} failed: {error}"),
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        let until_eof = play.seconds.is_none();
        // Enter stops the run; so does a closed stdin when nothing else would.
        thread::spawn(move || {
            let mut line = String::new();
            match std::io::stdin().lock().read_line(&mut line) {
                Ok(0) | Err(_) if !until_eof => {}
                _ => stop.store(true, Ordering::Relaxed),
            }
        });
    }
    let pace = Pace {
        null,
        buffer: play.buffer,
        fast: play.fast,
        limit: play.seconds.map(|s| (s * f64::from(rate)).round() as u64),
    };
    let mut audio = play.record_audio.is_some().then(Vec::new);
    drive(
        &mut host,
        &pace,
        &|| stop.load(Ordering::Relaxed),
        audio.as_mut(),
    );
    // An output the audio system stopped ends the run: what it recorded is
    // still written, and the command then fails with the reason.
    let fault = host.output_fault().map(str::to_owned);
    let Some(finished) = host.finish() else {
        return Ok(());
    };
    let mut live = finished.live;
    live.status(&finished.status);
    if let Err(error) = live.finish(finished.status.next_frame) {
        eprintln!("signal: the session's end was not recorded: {error}");
    }
    if let Some(dir) = &play.record_session {
        let mut recorder = live.into_recorder();
        recorder.set_devices(gpu.devices());
        recorder.write(dir).map_err(|error| match error {
            SessionError::Io { .. } => Failure::Io(error.to_string()),
            error => Failure::Internal(error.to_string()),
        })?;
    }
    if let (Some(path), Some(samples)) = (&play.record_audio, &audio) {
        write_wav(path, samples, width, rate)?;
    }
    if let Some(failure) = finished.status.failure {
        return Err(Failure::Internal(failure.to_string()));
    }
    if let Some(fault) = fault {
        return Err(Failure::Device(fault));
    }
    if json {
        print_stdout(
            &json!({
                "frames": finished.status.next_frame.0,
                "clipped": finished.clipped,
                "meter_overflows": finished.meter_overflows,
            })
            .to_string(),
        );
    } else {
        eprintln!(
            "signal: played {} frames at {rate} Hz; clipped samples {}",
            finished.status.next_frame.0, finished.clipped
        );
    }
    Ok(())
}

/// How a host plays a project (`signal ui`'s options, or `signal mcp`'s).
pub(crate) struct Played<'a> {
    /// The project folder, or a `.signal` file.
    pub(crate) project: &'a Path,
    /// The null driver (no device) instead of CoreAudio.
    pub(crate) null: bool,
    pub(crate) device: Option<String>,
    pub(crate) buffer: u32,
    pub(crate) threads: u32,
    pub(crate) rt: RtOptions,
    pub(crate) boundary_data: bool,
    /// The run records a session (or its audio): it is never restarted.
    pub(crate) recording: bool,
    /// The output ceiling (`signal mcp --max-gain`; `None`: none).
    pub(crate) ceiling: Option<f64>,
}

/// What a host needs to play a project, read and planned.
pub(crate) struct Serve {
    pub(crate) config: HostConfig,
    /// GPU-backed instances on this machine's devices, kept until the run
    /// is gone.
    pub(crate) gpu: Gpu,
    /// The project as the person named it.
    pub(crate) shown: PathBuf,
    /// The audio sink's channels.
    pub(crate) width: usize,
}

/// The UI library: the bundle's ui/ (Contents/Resources/ui, beside
/// Contents/Helpers), else the repository's the CLI was built from.
pub(crate) fn default_ui_dir() -> PathBuf {
    let exe = std::env::current_exe().ok();
    let built = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui"));
    signal_ui::library_dir(exe.as_deref(), built)
}

/// Loads and plans the project `played` names and makes its host's
/// configuration, as `signal ui` plays it (`server.project_ui` is set to the
/// project's own `ui/`).
pub(crate) fn serve(
    played: &Played,
    registry: &Registry,
    setup: Option<&Path>,
    mut server: ServerOptions,
) -> Result<Serve, Failure> {
    // A .signal file is served from a temporary folder, which the host keeps
    // (and its Setup field reopens, M2.8-12).
    let opened = project_arg(played.project)?;
    temporary_note(&opened);
    let (dir, shown) = (opened.dir().to_owned(), opened.shown().to_owned());
    let project = Project::load(&dir).map_err(|error| match error {
        LoadError::Invalid(diagnostics) => Failure::Invalid(located(&shown, diagnostics)),
        error @ LoadError::Io { .. } => Failure::Io(error.to_string()),
    })?;
    let plan = project
        .plan(registry)
        .map_err(|diagnostics| Failure::Invalid(located(&shown, diagnostics)))?
        .plan;
    let (_, width) = sink_of(&project, &plan).map_err(|d| Failure::Invalid(located(&shown, d)))?;
    // As `play --watch`: GPU-backed instances on this machine's devices,
    // kept until the run is gone.
    let mut gpu = Gpu::new(registry, true, project.gpu_pins(&plan), setup);
    for warning in gpu.warnings() {
        eprintln!("signal: {warning}");
    }
    let setup_path = setup_path(
        setup,
        std::env::var_os("SIGNAL_SETUP"),
        std::env::var_os("HOME"),
    )
    .map_err(Failure::Usage)?;
    // Under the null driver the Setup's devices are still the machine's,
    // so `setup.*` resolves them as Signal.app would.
    let backend = if played.null {
        Backend::Null(signal_audio::devices().unwrap_or_default())
    } else {
        Backend::CoreAudio
    };
    server.project_ui = project_ui(&dir);
    let config = HostConfig {
        dir: dir.clone(),
        source: shown.clone(),
        temporary: opened.into_temporary(),
        registry: gpu.registry().clone(),
        setup_path,
        backend,
        first: FirstRun {
            device: played.device.clone(),
            buffer: played.buffer,
            threads: played.threads,
        },
        rt: played.rt,
        boundary_data: played.boundary_data,
        recording: played.recording,
        provenance: provenance::collect(&provenance::source_dir(), &project.files),
        compute: compute_lines(),
        server,
        ceiling: played.ceiling,
    };
    Ok(Serve {
        config,
        gpu,
        shown,
        width,
    })
}

/// `signal ui check`: the binding check of every panel of the project.
fn check_panels(path: &Path, registry: &Registry, json: bool) -> Result<(), Failure> {
    let opened = project_arg(path)?;
    let (dir, shown) = (opened.dir(), opened.shown());
    let project = Project::load(dir).map_err(|error| match error {
        LoadError::Invalid(diagnostics) => Failure::Invalid(located(shown, diagnostics)),
        error @ LoadError::Io { .. } => Failure::Io(error.to_string()),
    })?;
    project
        .plan(registry)
        .map_err(|diagnostics| Failure::Invalid(located(shown, diagnostics)))?;
    let checked = check_project(dir, &project, registry).map_err(Failure::Io)?;
    let diagnostics: Vec<_> = checked
        .iter()
        .flat_map(|c| c.findings.iter().map(|f| f.diagnostic.clone()))
        .collect();
    let diagnostics = located(shown, diagnostics);
    if checked.iter().any(|c| c.failed()) {
        return Err(Failure::InvalidPanels(diagnostics));
    }
    if json {
        let panels: Vec<_> = checked
            .iter()
            .map(|c| json!({"file": c.file, "controls": c.controls, "bindings": c.bindings}))
            .collect();
        print_stdout(
            &serde_json::to_string_pretty(&json!({
                "panels": panels,
                "generated": !project_ui(dir).join(MAIN_PANEL).is_file(),
                "diagnostics": diagnostics,
            }))
            .unwrap_or_default(),
        );
        return Ok(());
    }
    for diagnostic in &diagnostics {
        eprintln!("{diagnostic}");
    }
    for c in &checked {
        eprintln!(
            "signal: {}: {} controls, {} bindings: ok",
            shown.join(&c.file).display(),
            c.controls,
            c.bindings
        );
    }
    if !project_ui(dir).join(MAIN_PANEL).is_file() {
        eprintln!(
            "signal: {}: no ui/{MAIN_PANEL}; the main panel is generated from the plan",
            shown.display()
        );
    }
    Ok(())
}
