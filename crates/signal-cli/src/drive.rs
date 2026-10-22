//! The control loop of a host that plays a project, shared by `signal ui`
//! and `signal mcp`: it steps the host (its clients, edits and watch) and,
//! under the null driver, runs the engine at real-time pace (`--fast`: as
//! fast as it goes). It never writes to stdout, which `signal mcp` keeps for
//! its protocol frames: the host's messages go to stderr.

#![deny(clippy::print_stdout)]

use std::thread;
use std::time::{Duration, Instant};

use signal_ui::Host;

/// How a host plays.
pub(crate) struct Pace {
    /// The null driver: this loop runs the engine.
    pub(crate) null: bool,
    /// Frames per null-driver chunk (the device's buffer otherwise).
    pub(crate) buffer: u32,
    /// The null driver as fast as it goes, instead of at real-time pace.
    pub(crate) fast: bool,
    /// Stop at this frame (`--seconds`).
    pub(crate) limit: Option<u64>,
}

/// Plays `host` until `pace.limit`, a failed engine, an output the audio
/// system stopped ([`Host::output_fault`]), or `stop()` (then once
/// every command sent has applied, or a second has passed, so an edit just
/// made is played and recorded). Under the null driver the samples played
/// are appended to `audio` when given.
pub(crate) fn drive(
    host: &mut Host,
    pace: &Pace,
    stop: &dyn Fn() -> bool,
    mut audio: Option<&mut Vec<f32>>,
) {
    let rate = host.sample_rate();
    let started = Instant::now();
    let mut played = 0u64;
    let mut stopping: Option<Instant> = None;
    loop {
        let status = host.controller().map(|c| (c.status(), c.in_flight()));
        // The null driver counts its chunks; a device, its runner's frames,
        // or the time since the start while nothing plays (a refused
        // restart, a `transport.stop`), so that `--seconds` always ends the
        // run.
        let frame = match (&status, pace.null) {
            (_, true) => played,
            (Some((s, _)), false) => s.next_frame.0,
            (None, false) => (started.elapsed().as_secs_f64() * f64::from(rate)) as u64,
        };
        // A failed engine plays only silence: stop (docs/semantics.md §14);
        // so does an output the audio system stopped (its rate changed, it
        // went away), which calls no more back.
        if pace.limit.is_some_and(|limit| frame >= limit)
            || status.as_ref().is_some_and(|(s, _)| s.failure.is_some())
            || host.output_fault().is_some()
        {
            break;
        }
        if stop() {
            let since = *stopping.get_or_insert_with(Instant::now);
            if status.is_none_or(|(_, in_flight)| in_flight == 0)
                || since.elapsed() > Duration::from_secs(1)
            {
                break;
            }
        }
        if pace.null {
            let chunk = pace.limit.map_or(u64::from(pace.buffer), |limit| {
                (limit - frame).min(u64::from(pace.buffer))
            });
            // Nothing runs after a refused restart or a `transport.stop`;
            // the chunk is counted anyway, so the run still ends at
            // `--seconds`, on time.
            let _ = host.run_null(chunk, |out| {
                if let Some(audio) = audio.as_deref_mut() {
                    audio.extend_from_slice(out);
                }
            });
            played += chunk;
            for message in host.step() {
                eprintln!("signal: {message}");
            }
            if !pace.fast {
                let due = started + Duration::from_secs_f64(played as f64 / f64::from(rate));
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    thread::sleep(wait);
                }
            }
        } else {
            let interval = Duration::from_secs_f64(f64::from(pace.buffer) / f64::from(rate))
                .min(Duration::from_millis(1));
            thread::sleep(interval);
            for message in host.step() {
                eprintln!("signal: {message}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};
    use signal_core::Registry;
    use signal_engine::RtOptions;
    use signal_ui::{Host, ServerOptions};
    use tungstenite::Message;

    use super::{Pace, drive};
    use crate::ui::{Played, default_ui_dir, serve};

    /// auditor-2-seconds-after-stop: under the null driver, `--seconds`
    /// still ends the run after a client's `transport.stop` (M2.12-1), on
    /// time: the chunks are counted whether a run plays or not.
    #[test]
    fn seconds_end_a_null_run_stopped_by_a_client() {
        let mut registry = Registry::new();
        signal_std::register(&mut registry).unwrap();
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sine");
        let played = Played {
            project: &project,
            null: true,
            device: None,
            buffer: 256,
            threads: 1,
            rt: RtOptions::default(),
            boundary_data: false,
            recording: false,
            ceiling: None,
        };
        let server = ServerOptions {
            port: 0,
            remote: false,
            ui_dir: default_ui_dir(),
            project_ui: project.join("ui"),
            client_queue: 16,
        };
        let mut serve = serve(&played, &registry, None, server).ok().unwrap();
        serve.config.setup_path = std::env::temp_dir().join(format!(
            "signal-drive-test-setup-{}.toml",
            std::process::id()
        ));
        let mut host = Host::new_local(serve.config).unwrap();
        let (connector, token) = (host.local_connector(), host.token().to_owned());
        let client = std::thread::spawn(move || {
            let (mut ws, _) = tungstenite::client("ws://localhost/", connector.connect()).unwrap();
            let hello = json!({"type": "hello", "protocol": 1, "token": token});
            ws.send(Message::text(hello.to_string())).unwrap();
            let stop = json!({"type": "transport.stop", "id": 1});
            ws.send(Message::text(stop.to_string())).unwrap();
            loop {
                if let Message::Text(text) = ws.read().unwrap() {
                    let reply: Value = serde_json::from_str(text.as_str()).unwrap();
                    if reply["id"] == 1 {
                        return reply;
                    }
                }
            }
        });
        let rate = host.sample_rate();
        let pace = Pace {
            null: true,
            buffer: 256,
            fast: false,
            limit: Some(u64::from(rate)),
        };
        let started = std::time::Instant::now();
        drive(&mut host, &pace, &|| false, None);
        let elapsed = started.elapsed();
        let reply = client.join().unwrap();
        assert_eq!(reply["playing"], false, "{reply}");
        assert!(host.controller().is_none(), "stopped by the client");
        assert!(
            elapsed.as_secs_f64() < 3.0,
            "one second of frames ended the run after the stop: {elapsed:?}"
        );
    }
}
