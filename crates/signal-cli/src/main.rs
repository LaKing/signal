//! The `signal` command (headless entry point; ADR 0013): `render`, `replay`,
//! `play`, `bench`, `devices`, `modules` and `schema`. `--json` makes every command print machine-readable output on
//! stdout. Exit codes: 0 ok, 1 validation or user error, 2 internal error.
//! The command layer only: sessions, live edits and provenance live in
//! `signal-project`, the audio device in `signal-audio`.

mod bench;
mod devices;
mod play;
mod render;
mod replay;

use std::io::Write;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use serde_json::json;
use signal_core::{Direction, Feedthrough, ModuleDescriptor, Registry};
use signal_project::Diagnostic;

/// Signal: render, replay and inspect projects.
#[derive(Parser)]
#[command(name = "signal", version)]
struct Cli {
    /// Print machine-readable JSON on stdout.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render a project offline: audio.wav, manifest.json, log.jsonl, metrics.json.
    Render(render::RenderArgs),
    /// Replay a recorded session directory offline: the same artifacts as render.
    Replay(replay::ReplayArgs),
    /// Play a project in real time (until Enter, or --seconds), with live
    /// edits (--watch) and session recording; statistics at the end.
    Play(play::PlayArgs),
    /// Measure the per-frame engine at 96 kHz (V13): every `std.*`
    /// primitive, the empty engine, the kick project and kick voices.
    Bench(bench::BenchArgs),
    /// List the audio devices with their sample rates, channels and buffer sizes.
    Devices,
    /// Print the catalogue of module descriptors.
    Modules,
    /// Print the JSON Schema of project and composite files.
    Schema,
}

/// Why a command failed.
pub(crate) enum Failure {
    /// Bad command-line arguments.
    Usage(String),
    /// The project is invalid.
    Invalid(Vec<Diagnostic>),
    /// The session directory is malformed (`signal replay`).
    InvalidSession(Vec<Diagnostic>),
    /// A file could not be read or written.
    Io(String),
    /// The audio device cannot play the project (exit 1).
    Device(String),
    /// A valid session cannot be replayed as asked (exit 1): its boundary
    /// records are incomplete, or a recorded result cannot be reproduced.
    Replay(String),
    /// A bug: something that validation should have ruled out.
    Internal(String),
}

impl Failure {
    fn exit_code(&self) -> ExitCode {
        match self {
            Failure::Internal(_) => ExitCode::from(2),
            _ => ExitCode::from(1),
        }
    }

    fn report(&self, json: bool) {
        let (kind, message, diagnostics) = match self {
            Failure::Usage(message) => ("usage", message.clone(), &[][..]),
            Failure::Invalid(diagnostics) => (
                "validation",
                format!("the project is invalid ({} errors)", diagnostics.len()),
                diagnostics.as_slice(),
            ),
            Failure::InvalidSession(diagnostics) => (
                "validation",
                format!("the session is invalid ({} errors)", diagnostics.len()),
                diagnostics.as_slice(),
            ),
            Failure::Io(message) => ("io", message.clone(), &[][..]),
            Failure::Device(message) => ("device", message.clone(), &[][..]),
            Failure::Replay(message) => ("replay", message.clone(), &[][..]),
            Failure::Internal(message) => ("internal", message.clone(), &[][..]),
        };
        if json {
            let output = json!({
                "error": { "kind": kind, "message": message, "diagnostics": diagnostics }
            });
            print_stdout(&serde_json::to_string_pretty(&output).unwrap_or_default());
        } else if diagnostics.is_empty() {
            eprintln!("signal: {message}");
        } else {
            for diagnostic in diagnostics {
                eprintln!("{diagnostic}");
            }
        }
    }
}

/// Prints `text` and a newline on stdout, ignoring a closed stdout.
pub(crate) fn print_stdout(text: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{text}");
}

fn main() -> ExitCode {
    let json = std::env::args_os().any(|arg| arg == "--json");
    match std::panic::catch_unwind(|| run(json)) {
        Ok(code) => code,
        Err(_) => {
            let failure = Failure::Internal("internal error (panic)".to_owned());
            failure.report(json);
            failure.exit_code()
        }
    }
}

fn run(json: bool) -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                let _ = error.print();
                return ExitCode::SUCCESS;
            }
            if json {
                Failure::Usage(error.to_string().trim().to_owned()).report(true);
            } else {
                let _ = error.print();
            }
            return ExitCode::from(1);
        }
    };
    match execute(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            failure.report(cli.json);
            failure.exit_code()
        }
    }
}

fn execute(cli: &Cli) -> Result<(), Failure> {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).map_err(|e| Failure::Internal(e.to_string()))?;
    // ADR 0015: `lab.*` only in a `--features lab` build.
    #[cfg(feature = "lab")]
    signal_lab::register(&mut registry).map_err(|e| Failure::Internal(e.to_string()))?;
    match &cli.command {
        Command::Render(args) => render::run(args, &registry, cli.json),
        Command::Replay(args) => replay::run(args, &registry, cli.json),
        Command::Play(args) => play::run(args, &registry, cli.json),
        Command::Bench(args) => bench::run(args, &registry, cli.json),
        Command::Devices => devices::run(cli.json),
        Command::Modules => {
            if cli.json {
                let descriptors: Vec<ModuleDescriptor> =
                    registry.iter().map(|t| t.descriptor).collect();
                let text = serde_json::to_string_pretty(&descriptors)
                    .map_err(|e| Failure::Internal(e.to_string()))?;
                print_stdout(&text);
            } else {
                let blocks: Vec<String> =
                    registry.iter().map(|t| describe(&t.descriptor)).collect();
                print_stdout(&blocks.join("\n"));
            }
            Ok(())
        }
        Command::Schema => {
            let schema = signal_project::schema(&registry);
            let text = serde_json::to_string_pretty(&schema)
                .map_err(|e| Failure::Internal(e.to_string()))?;
            print_stdout(&text);
            Ok(())
        }
    }
}

/// A module descriptor for people: name, class, doc, ports and parameters.
fn describe(d: &ModuleDescriptor) -> String {
    let class = match d.feedthrough {
        Feedthrough::Direct => "direct",
        Feedthrough::StateOutput => "state-output",
    };
    let sink = if d.sink.is_some() { ", sink" } else { "" };
    // ADR 0019 §3: a declared latency and the reproducibility class.
    let latency = d.latency.map_or(String::new(), |l| {
        format!(
            ", {} blocks of {} frames late",
            l.latency_blocks, l.block_frames
        )
    });
    let machine = if d.reproducibility.is_bit_exact() {
        ""
    } else {
        ", same_machine"
    };
    let mut text = format!(
        "{} ({class}{sink}{latency}{machine})\n  {}\n",
        d.type_name, d.doc
    );
    for port in d.ports {
        let direction = match port.direction {
            Direction::In => "in ",
            Direction::Out => "out",
        };
        let unit = port.unit.map(|u| format!(" [{u}]")).unwrap_or_default();
        text.push_str(&format!(
            "  {direction} {} ({:?}, {:?}){unit}\n",
            port.name, port.shape, port.kind
        ));
    }
    for p in d.parameters {
        let unit = p.unit.map(|u| format!(" [{u}]")).unwrap_or_default();
        text.push_str(&format!(
            "  parameter {} ({:?}){unit}: {} to {}, default {}, {:?}\n",
            p.name, p.kind, p.range.min, p.range.max, p.default, p.combine
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::Failure;
    use std::process::ExitCode;

    #[test]
    fn a_device_that_cannot_play_the_project_exits_1() {
        let error = signal_audio::AudioError::SampleRate {
            device: "Speakers".to_owned(),
            requested: 96_000,
            supported: vec![(44_100, 44_100), (48_000, 48_000)],
        };
        let failure = Failure::Device(error.to_string());
        assert_eq!(failure.exit_code(), ExitCode::from(1));
    }
}
