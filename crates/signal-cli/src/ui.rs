//! `signal ui`: plays a project as `signal play --watch` does, with the same
//! driver options, and serves it to web pages through the UI protocol
//! (`signal-ui`, `docs/ui-protocol.md`): the UI files over HTTP and a
//! WebSocket, on 127.0.0.1 unless `--remote`. `--open` opens the page in the
//! default browser. This thread is the control thread: it steps the host.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use clap::Args;
use serde_json::json;
use signal_core::Registry;
use signal_engine::{ControlSink, RtOptions};
use signal_project::session::{SessionError, sink_of};
use signal_project::{LoadError, Project, provenance};
use signal_ui::player::Backend;
use signal_ui::setup::setup_path;
use signal_ui::{FirstRun, Host, HostConfig, ServerOptions, project_ui};

use crate::devices::compute_lines;
use crate::gpu::Gpu;
use crate::play::{DriverKind, PlayArgs, check, write_wav};
use crate::render::located;
use crate::{Failure, print_stdout};

/// Each client's queue of outgoing messages.
const CLIENT_QUEUE: usize = 256;

/// Arguments of `signal ui`.
#[derive(Args)]
pub(crate) struct UiArgs {
    #[command(flatten)]
    play: PlayArgs,
    /// Open the page in the default browser.
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
    /// The UI library folder (default: the repository's `ui/`).
    #[arg(long)]
    ui_dir: Option<PathBuf>,
}

pub(crate) fn run(
    args: &UiArgs,
    registry: &Registry,
    setup: Option<&Path>,
    json: bool,
) -> Result<(), Failure> {
    let play = &args.play;
    check(play)?;
    let null = play.driver == DriverKind::Null;
    let dir = &play.project;
    let project = Project::load(dir).map_err(|error| match error {
        LoadError::Invalid(diagnostics) => Failure::Invalid(located(dir, diagnostics)),
        error @ LoadError::Io { .. } => Failure::Io(error.to_string()),
    })?;
    let plan = project
        .plan(registry)
        .map_err(|diagnostics| Failure::Invalid(located(dir, diagnostics)))?
        .plan;
    let (_, width) = sink_of(&project, &plan).map_err(|d| Failure::Invalid(located(dir, d)))?;
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
    let backend = if null {
        Backend::Null(signal_audio::devices().unwrap_or_default())
    } else {
        Backend::CoreAudio
    };
    let ui_dir = args
        .ui_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui")));
    let config = HostConfig {
        dir: dir.clone(),
        registry: gpu.registry().clone(),
        setup_path,
        backend,
        first: FirstRun {
            device: play.device.clone(),
            buffer: play.buffer,
            threads: play.threads,
        },
        rt: RtOptions {
            policy: play.rt_policy,
            computation: play.rt_computation,
            trace: play.rt_trace,
        },
        boundary_data: play.record_boundary_data,
        recording: play.record_session.is_some() || play.record_audio.is_some(),
        provenance: provenance::collect(&provenance::source_dir(), &project.files),
        compute: compute_lines(),
        server: ServerOptions {
            port: args.port,
            remote: args.remote,
            ui_dir,
            project_ui: project_ui(dir),
            client_queue: CLIENT_QUEUE,
        },
    };
    let mut host = Host::new(config).map_err(Failure::Device)?;
    let rate = host.sample_rate();
    let url = host.url();
    eprintln!("signal: ui at {url}");
    if json {
        print_stdout(
            &json!({
                "url": url,
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
        dir.display(),
        if null { " with the null driver" } else { "" }
    );
    if args.open {
        match std::process::Command::new("/usr/bin/open")
            .arg(&url)
            .status()
        {
            Ok(status) if status.success() => {}
            Ok(status) => eprintln!("signal: warning: open {url} failed ({status})"),
            Err(error) => eprintln!("signal: warning: open {url} failed: {error}"),
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
    let limit = play.seconds.map(|s| (s * f64::from(rate)).round() as u64);
    let mut audio = play.record_audio.is_some().then(Vec::new);
    let started = Instant::now();
    let mut played = 0u64;
    let mut stopping: Option<Instant> = None;
    loop {
        let status = host.controller().map(|c| (c.status(), c.in_flight()));
        // The null driver counts its chunks; a device, its runner's frames,
        // or the time since the start while nothing plays (a refused
        // restart), so that `--seconds` always ends the run.
        let frame = match (&status, null) {
            (_, true) => played,
            (Some((s, _)), false) => s.next_frame.0,
            (None, false) => (started.elapsed().as_secs_f64() * f64::from(rate)) as u64,
        };
        // A failed engine plays only silence: stop (docs/semantics.md §14).
        if limit.is_some_and(|limit| frame >= limit)
            || status.as_ref().is_some_and(|(s, _)| s.failure.is_some())
        {
            break;
        }
        // Stopped: once every command sent has applied (or a second has
        // passed), so an edit just made is played and recorded.
        if stop.load(Ordering::Relaxed) {
            let since = *stopping.get_or_insert_with(Instant::now);
            if status.is_none_or(|(_, in_flight)| in_flight == 0)
                || since.elapsed() > Duration::from_secs(1)
            {
                break;
            }
        }
        if null {
            let chunk = limit.map_or(u64::from(play.buffer), |limit| {
                (limit - frame).min(u64::from(play.buffer))
            });
            // Nothing runs after a refused restart; the chunk is counted
            // anyway, so the run still ends at `--seconds`, on time.
            let _ = host.run_null(chunk, |out| {
                if let Some(audio) = audio.as_mut() {
                    audio.extend_from_slice(out);
                }
            });
            played += chunk;
            for message in host.step() {
                eprintln!("signal: {message}");
            }
            if !play.fast {
                let due = started + Duration::from_secs_f64(played as f64 / f64::from(rate));
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    thread::sleep(wait);
                }
            }
        } else {
            let interval = Duration::from_secs_f64(f64::from(play.buffer) / f64::from(rate))
                .min(Duration::from_millis(1));
            thread::sleep(interval);
            for message in host.step() {
                eprintln!("signal: {message}");
            }
        }
    }
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
