//! `signal-app`: Signal.app's window (ADR 0013, 0023), a tao window with a
//! wry WebView on the app's own UI server ([`AppServer`]). `--setup <file>`
//! and `--project <dir>` choose the Setup file and the project to open, for
//! tests and scripting.
//!
//! The page holds the app's menu (Open project, Interface setup, Quit; tao
//! has no menu bar and a native one would need `unsafe`, ADR 0023). Quit
//! reaches this side through wry's IPC; a folder dropped on the window is
//! handed to the page, which opens it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use signal_app::server::{AppConfig, AppServer};
use signal_app::setup::{Setup, setup_path};
use signal_app::state::Backend;
use signal_app::window::{Action, dropped, ipc_action, may_navigate, origin_of, window_of};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::{DragDropEvent, WebViewBuilder};

/// Signal.app: interface setup, play and stop, meters, panels.
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

/// What the page and the WebView tell the event loop.
#[derive(Debug)]
enum UserEvent {
    /// The menu's Quit.
    Quit,
    /// What was dropped on the window (a folder is opened).
    Dropped(PathBuf),
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
    // Absolute, because it becomes the Setup's last project, and
    // Signal.app opened from the Finder starts in `/`.
    let project = args
        .project
        .map(|dir| std::path::absolute(&dir).unwrap_or(dir));
    // The bundle's ui/ (Contents/Resources/ui), else the repository's.
    let exe = std::env::current_exe().ok();
    let built = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui"));
    let server = AppServer::start(AppConfig {
        setup_path: path,
        project,
        backend: Backend::CoreAudio,
        ui_dir: signal_ui::library_dir(exe.as_deref(), built),
    });
    let server = match server {
        Ok(server) => server,
        Err(error) => {
            eprintln!("signal-app: {error}");
            return ExitCode::FAILURE;
        }
    };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let mut builder = WindowBuilder::new().with_title("Signal");
    if let Some(window) = setup.window {
        builder = builder.with_inner_size(LogicalSize::new(
            f64::from(window.width),
            f64::from(window.height),
        ));
    }
    let window = match builder.build(&event_loop) {
        Ok(window) => window,
        Err(error) => {
            eprintln!("signal-app: the window did not open: {error}");
            return ExitCode::FAILURE;
        }
    };
    let url = server.url().to_owned();
    let origin = origin_of(&url).to_owned();
    let quit = event_loop.create_proxy();
    let drops = event_loop.create_proxy();
    let webview = WebViewBuilder::new()
        .with_url(url)
        .with_navigation_handler(move |to| may_navigate(&to, &origin))
        .with_ipc_handler(move |request| {
            if ipc_action(request.body()) == Some(Action::Quit) {
                let _ = quit.send_event(UserEvent::Quit);
            }
        })
        .with_drag_drop_handler(move |event| {
            if let DragDropEvent::Drop { paths, .. } = event
                && let Some(path) = paths.into_iter().next()
            {
                let _ = drops.send_event(UserEvent::Dropped(path));
            }
            // The page opens folders; nothing is navigated to.
            true
        })
        .build(&window);
    let webview = match webview {
        Ok(webview) => webview,
        Err(error) => {
            eprintln!("signal-app: the WebView did not open: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut server = Some(server);
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let close = match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
            | Event::UserEvent(UserEvent::Quit) => true,
            Event::UserEvent(UserEvent::Dropped(path)) => {
                // A folder is opened; anything else is refused, as before.
                let (event, text) = match dropped(&path) {
                    Ok(dir) => ("signal-drop", dir.display().to_string()),
                    Err(refusal) => ("signal-app-error", refusal.to_owned()),
                };
                let quoted = serde_json::to_string(&text).unwrap_or_else(|_| "\"\"".to_owned());
                let script = format!(
                    "window.dispatchEvent(new CustomEvent('{event}', {{ detail: {quoted} }}))"
                );
                if let Err(error) = webview.evaluate_script(&script) {
                    eprintln!("signal-app: {error}");
                }
                false
            }
            _ => false,
        };
        if close {
            // The size the window has now, in points, goes into the Setup.
            let size = window.inner_size().to_logical::<f64>(window.scale_factor());
            if let Some(mut server) = server.take() {
                server.exit(window_of(size.width, size.height));
            }
            *control_flow = ControlFlow::Exit;
        }
    })
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
