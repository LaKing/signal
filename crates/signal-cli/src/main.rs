//! The `signal` command (headless entry point; ADR 0013): `render`, `replay`,
//! `play`, `ui`, `mcp`, `pack`, `unpack`, `vendor`, `snapshot`, `bench`, `devices`, `modules` and `schema`. `--json` makes every command print machine-readable output on
//! stdout. Exit codes: 0 ok, 1 validation or user error, 2 internal error.
//! The command layer only: sessions, live edits and provenance live in
//! `signal-project`, the audio device in `signal-audio`.

mod bench;
mod devices;
mod drive;
mod files;
mod gpu;
mod mcp;
mod play;
mod render;
mod replay;
mod snapshot;
mod ui;
mod vendor;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use signal_core::{Direction, Feedthrough, ModuleDescriptor, Registry};
use signal_project::Diagnostic;

/// The version: `SIGNAL_VERSION` when the build set it
/// (`scripts/version.sh`, as `scripts/bundle-app.sh` does), else the
/// workspace version.
const VERSION: &str = match option_env!("SIGNAL_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// Signal: render, replay and inspect projects.
#[derive(Parser)]
#[command(name = "signal", version = VERSION)]
struct Cli {
    /// Print machine-readable JSON on stdout.
    #[arg(long, global = true)]
    json: bool,
    /// The machine Setup (default: $SIGNAL_SETUP, else Signal.app's
    /// Setup): `signal ui` reads and writes it (`setup.*`); in a GPU build
    /// its `gpu_devices` limits the GPU devices.
    #[arg(long, global = true)]
    setup: Option<PathBuf>,
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
    /// Play a project as `play --watch` does (--watch is implied: the folder
    /// is always watched) and serve it to web pages through the UI protocol
    /// (docs/ui-protocol.md), on 127.0.0.1; --open opens the project's main
    /// panel in the default browser. `signal ui check <project>` checks the
    /// panels' bindings instead.
    Ui(ui::UiArgs),
    /// Run an MCP (Model Context Protocol) server over stdio, so an LLM client
    /// can do what a person at the panel can (ADR 0029,
    /// docs/agent-interface.md); stdout carries protocol frames only.
    #[command(long_about = mcp::LONG_ABOUT)]
    Mcp(mcp::McpArgs),
    /// Pack a project folder into one .signal file (a deterministic zip of
    /// the files a project reads; docs/project-format.md §10).
    Pack(files::PackArgs),
    /// Unpack a .signal file into a new folder; every entry is checked
    /// first, and a refused file writes nothing.
    Unpack(files::UnpackArgs),
    /// Copy a library composite (and the library composites it uses) into a
    /// project's composites/, as the project's own text, and point the
    /// project's references at the copies (ADR 0027 §4).
    Vendor(vendor::VendorArgs),
    /// A project's snapshots (ADR 0041): list, save, restore, undo or delete
    /// the values of its top-level parameters stored under a name in
    /// snapshots.toml, and exclude a control from them.
    Snapshot(snapshot::SnapshotArgs),
    /// Measure the per-frame engine at 96 kHz (V13): every `std.*`
    /// primitive, the empty engine, the kick project and kick voices.
    Bench(bench::BenchArgs),
    /// List the audio devices with their sample rates, channels and buffer
    /// sizes; with --compute, the compute devices instead.
    Devices(devices::DevicesArgs),
    /// Print the catalogue of module descriptors and library composites.
    Modules(ModulesArgs),
    /// Print the JSON Schema of project and composite files.
    Schema,
}

/// Arguments of `signal modules`.
#[derive(Args)]
struct ModulesArgs {
    /// Only the types of this palette group (a descriptor's `category`, e.g.
    /// `core`, `filter`; ADR 0027 §1).
    #[arg(long)]
    category: Option<String>,
    /// List the library's slot interface templates instead (`lib.voice`,
    /// `lib.instrument`, `lib.deck`; ADR 0037 §3).
    #[arg(long, conflicts_with = "category")]
    interfaces: bool,
}

/// Why a command failed.
pub(crate) enum Failure {
    /// Bad command-line arguments.
    Usage(String),
    /// The project is invalid.
    Invalid(Vec<Diagnostic>),
    /// A `.signal` file was refused (M2.8-12): its reason.
    Archive(String),
    /// The session directory is malformed (`signal replay`).
    InvalidSession(Vec<Diagnostic>),
    /// A panel binds what the plan does not have (`signal ui check`).
    InvalidPanels(Vec<Diagnostic>),
    /// A file could not be read or written.
    Io(String),
    /// The audio device cannot play the project (exit 1).
    Device(String),
    /// A valid session cannot be replayed as asked (exit 1): its boundary
    /// records are incomplete, or a recorded result cannot be reproduced.
    Replay(String),
    /// An exact replay is not the performance (exit 2, M2.7-8): a recomputed
    /// or injected result differs from its recorded hash.
    Mismatch(String),
    /// A bug: something that validation should have ruled out.
    Internal(String),
}

impl Failure {
    fn exit_code(&self) -> ExitCode {
        match self {
            Failure::Internal(_) | Failure::Mismatch(_) => ExitCode::from(2),
            _ => ExitCode::from(1),
        }
    }

    /// The failure as one text: its message, then its diagnostics, one per
    /// line (`signal mcp`'s tool errors).
    pub(crate) fn message(&self) -> String {
        let (_, message, diagnostics) = self.parts();
        std::iter::once(message)
            .chain(diagnostics.iter().map(ToString::to_string))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn report(&self, json: bool) {
        let (kind, message, diagnostics) = self.parts();
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

    /// Its kind, message and diagnostics.
    fn parts(&self) -> (&'static str, String, &[Diagnostic]) {
        match self {
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
            Failure::InvalidPanels(diagnostics) => (
                "validation",
                format!(
                    "the panels have {} errors",
                    diagnostics
                        .iter()
                        .filter(|d| d.severity == signal_project::Severity::Error)
                        .count()
                ),
                diagnostics.as_slice(),
            ),
            Failure::Io(message) => ("io", message.clone(), &[][..]),
            Failure::Archive(message) => ("archive", message.clone(), &[][..]),
            Failure::Device(message) => ("device", message.clone(), &[][..]),
            Failure::Replay(message) => ("replay", message.clone(), &[][..]),
            Failure::Mismatch(message) => ("replay-mismatch", message.clone(), &[][..]),
            Failure::Internal(message) => ("internal", message.clone(), &[][..]),
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
    #[cfg(feature = "gpu")]
    let setup = cli.setup.as_deref();
    #[cfg(not(feature = "gpu"))]
    let setup = None;
    match &cli.command {
        Command::Render(args) => render::run(args, &registry, setup, cli.json),
        Command::Replay(args) => replay::run(args, &registry, setup, cli.json),
        Command::Play(args) => play::run(args, &registry, setup, cli.json),
        Command::Ui(args) => ui::run(args, &registry, cli.setup.as_deref(), cli.json),
        Command::Mcp(args) => mcp::run(args, &registry, cli.setup.as_deref()),
        Command::Pack(args) => files::pack(args, cli.json),
        Command::Unpack(args) => files::unpack(args, cli.json),
        Command::Vendor(args) => vendor::run(args, cli.json),
        Command::Snapshot(args) => snapshot::run(args, &registry, cli.json),
        Command::Bench(args) => bench::run(args, &registry, setup, cli.json),
        Command::Devices(args) => devices::run(args, cli.json),
        Command::Modules(args) if args.interfaces => {
            let templates = signal_project::library_interfaces();
            let text = if cli.json {
                serde_json::to_string_pretty(&templates)
                    .map_err(|e| Failure::Internal(e.to_string()))?
            } else {
                templates
                    .iter()
                    .map(describe_interface)
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            print_stdout(&text);
            Ok(())
        }
        Command::Modules(args) => {
            let (descriptors, library) = modules(&registry, args.category.as_deref())?;
            if cli.json {
                let mut all = serde_json::to_value(&descriptors)
                    .map_err(|e| Failure::Internal(e.to_string()))?;
                if let Value::Array(entries) = &mut all {
                    entries.extend(library);
                }
                let text = serde_json::to_string_pretty(&all)
                    .map_err(|e| Failure::Internal(e.to_string()))?;
                print_stdout(&text);
            } else {
                let blocks: Vec<String> = descriptors
                    .iter()
                    .map(describe)
                    .chain(library.iter().map(describe_library))
                    .collect();
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

/// The registry's descriptors and the library composites (ADR 0027 §4,
/// each `{"type_name": "lib.<name>", "library": true, …}`, a validation
/// reference with `"reference_for": "std.<type>"`), or with
/// `category` only those of that palette group; a group none has is a usage
/// error naming those there are.
fn modules(
    registry: &Registry,
    category: Option<&str>,
) -> Result<(Vec<ModuleDescriptor>, Vec<Value>), Failure> {
    let library: Vec<Value> = signal_project::library_modules(registry)
        .map_err(|d| Failure::Internal(format!("the built-in library does not load: {d:?}")))?
        .into_iter()
        .map(|mut entry| {
            if let Value::Object(fields) = &mut entry {
                let name = fields.remove("module").unwrap_or(Value::Null);
                let doc = fields.remove("description").unwrap_or(Value::Null);
                fields.remove("composite");
                fields.insert("type_name".to_owned(), name);
                fields.insert("doc".to_owned(), doc);
            }
            entry
        })
        .collect();
    let all = registry.iter().map(|t| t.descriptor);
    let Some(category) = category else {
        return Ok((all.collect(), library));
    };
    let chosen: Vec<ModuleDescriptor> = all.filter(|d| d.category == Some(category)).collect();
    let library_chosen: Vec<Value> = library
        .iter()
        .filter(|e| e["category"] == category)
        .cloned()
        .collect();
    if chosen.is_empty() && library_chosen.is_empty() {
        let mut known: Vec<&str> = registry
            .iter()
            .filter_map(|t| t.descriptor.category)
            .chain(library.iter().filter_map(|e| e["category"].as_str()))
            .collect();
        known.sort_unstable();
        known.dedup();
        return Err(Failure::Usage(format!(
            "no module type has category `{category}`; categories: {}",
            known.join(", ")
        )));
    }
    Ok((chosen, library_chosen))
}

/// A library composite for people, in the form [`describe`] gives a
/// primitive type: name, description, category, ports (shape, kind, unit)
/// and parameters (kind, unit, range, the composite's own default, combine).
/// A library interface template, as `signal modules --interfaces` prints it.
fn describe_interface(template: &signal_project::LibraryInterface) -> String {
    let mut text = format!(
        "{} (interface template, hash {})\n  {}\n",
        template.name,
        template.hash,
        template
            .interface
            .description
            .as_deref()
            .unwrap_or_default()
    );
    for port in &template.interface.ports {
        let role = match port.role {
            signal_project::Role::Input => "in   ",
            signal_project::Role::Output => "out  ",
            signal_project::Role::Parameter => "param",
        };
        let shape = match port.shape {
            signal_core::PortShape::Events => "events".to_owned(),
            signal_core::PortShape::Vector(n) => format!("vector {n}"),
            _ => "signal".to_owned(),
        };
        let unit = port
            .unit
            .as_ref()
            .map(|u| format!(" [{u}]"))
            .unwrap_or_default();
        text.push_str(&format!(
            "  {role} {} {shape} {:?}{unit}\n",
            port.name, port.kind
        ));
    }
    text
}

fn describe_library(entry: &Value) -> String {
    let text_of = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    // `"audio"` as the descriptors' `Audio`, `"add"` as `Add`.
    let named = |v: &Value| {
        text_of(v)
            .split('_')
            .map(|word| {
                let mut chars = word.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_uppercase().chain(chars).collect()
                })
            })
            .collect::<String>()
    };
    let unit = |v: &Value| v.as_str().map(|u| format!(" [{u}]")).unwrap_or_default();
    let number = |v: &Value| v.as_f64().map_or_else(|| v.to_string(), |x| x.to_string());
    let mut text = format!(
        "{} (library composite, hash {})\n  {}\n  category: {}\n",
        text_of(&entry["type_name"]),
        text_of(&entry["hash"]),
        text_of(&entry["doc"]),
        text_of(&entry["category"])
    );
    if let Some(native) = entry["reference_for"].as_str() {
        text.push_str(&format!(
            "  reference for: {native} (a validation reference, not a building block)\n"
        ));
    }
    for port in entry["ports"].as_array().into_iter().flatten() {
        let direction = if port["direction"] == "in" {
            "in "
        } else {
            "out"
        };
        text.push_str(&format!(
            "  {direction} {} ({}, {}){}\n",
            text_of(&port["name"]),
            named(&port["shape"]),
            named(&port["kind"]),
            unit(&port["unit"])
        ));
    }
    for p in entry["parameters"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "  parameter {} ({}){}: {} to {}, default {}, {}\n",
            text_of(&p["name"]),
            named(&p["kind"]),
            unit(&p["unit"]),
            number(&p["range"]["min"]),
            number(&p["range"]["max"]),
            number(&p["default"]),
            named(&p["combine"])
        ));
    }
    text
}

/// A module descriptor for people: name, class, doc, category, ports and
/// parameters.
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
    if let Some(category) = d.category {
        text.push_str(&format!("  category: {category}\n"));
    }
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

    /// C4: an exact replay that is not the performance exits 2; a
    /// provenance refusal, an exact replay that cannot be made, exits 1.
    #[test]
    fn a_replay_mismatch_exits_2() {
        assert_eq!(
            Failure::Mismatch(String::new()).exit_code(),
            ExitCode::from(2)
        );
        assert_eq!(
            Failure::Replay(String::new()).exit_code(),
            ExitCode::from(1)
        );
    }
}
