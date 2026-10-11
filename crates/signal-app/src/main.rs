//! `signal-app`: Signal.app's window (ADR 0013, 0018). `--setup <file>` and
//! `--project <dir>` choose the Setup file and the project to open, for
//! tests and scripting.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use eframe::egui;
use signal_app::control::Control;
use signal_app::setup::{Setup, setup_path};
use signal_app::state::{AppState, Backend};
use signal_app::ui::SignalApp;

/// Signal.app: interface setup, play and stop, meters.
#[derive(Parser, Debug, PartialEq)]
#[command(name = "signal-app")]
struct Args {
    /// The Setup file (else SIGNAL_SETUP, else
    /// ~/Library/Application Support/Signal/setup.toml).
    #[arg(long)]
    setup: Option<PathBuf>,
    /// The project folder to open (else the Setup's last project).
    #[arg(long)]
    project: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let path = match setup_path(
        args.setup.as_deref(),
        std::env::var_os("SIGNAL_SETUP"),
        std::env::var_os("HOME"),
    ) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("signal-app: {message}");
            return ExitCode::from(2);
        }
    };
    // Read here for the window's size and to report problems on the
    // terminal too; the control thread reads it again as its own.
    let (setup, problems) = Setup::load(&path);
    for problem in &problems {
        eprintln!("signal-app: {problem}");
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Signal")
        .with_drag_and_drop(true);
    if let Some(window) = setup.window {
        viewport = viewport.with_inner_size([window.width, window.height]);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let result = eframe::run_native(
        "Signal",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let project = args.project;
            let control = Control::spawn(
                move || AppState::new(path, project, Backend::CoreAudio),
                move || ctx.request_repaint(),
            )?;
            Ok(Box::new(SignalApp::new(control)))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("signal-app: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Args;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn setup_and_project_flags() {
        let args = Args::try_parse_from([
            "signal-app",
            "--setup",
            "s.toml",
            "--project",
            "projects/kick",
        ])
        .unwrap();
        assert_eq!(
            args,
            Args {
                setup: Some(PathBuf::from("s.toml")),
                project: Some(PathBuf::from("projects/kick")),
            }
        );
        assert_eq!(
            Args::try_parse_from(["signal-app"]).unwrap(),
            Args {
                setup: None,
                project: None
            }
        );
    }
}
