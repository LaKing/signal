//! `signal-app`: Signal.app's window (ADR 0013, 0023), a tao window with a
//! wry WebView on the app's own UI server ([`AppServer`]). `--setup <file>`
//! and `--project <dir>` choose the Setup file and the project to open, for
//! tests and scripting.
//!
//! The menu bar is native (`muda`, M2.8-11): the app menu, File (Open
//! Project…, Save Project, Interface Setup…, Quit) and Edit, so the page's
//! text fields get the system's shortcuts. Quit closes the window here;
//! Open, Save and Interface Setup are handed to the page, which runs them
//! (a `signal-menu` event). A folder dropped on the window is handed to the
//! page, which opens it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use muda::accelerator::{CMD_OR_CTRL, Key, KeyAccelerator};
use muda::{AboutMetadata, IsMenuItem, Menu, MenuEvent, PredefinedMenuItem, Submenu};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
use objc2_foundation::NSString;
use signal_app::server::{AppConfig, AppServer};
use signal_app::setup::{Setup, setup_path};
use signal_app::state::Backend;
use signal_app::window::{
    MENU_BAR, MenuAction, MenuItem, NATIVE_MENU_SCRIPT, dropped, may_navigate, origin_of, window_of,
};
use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause, WindowEvent};
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

/// What the menu bar and the WebView tell the event loop.
#[derive(Debug)]
enum UserEvent {
    /// One of Signal's own menu items.
    Menu(MenuAction),
    /// What was dropped on the window (a folder is opened).
    Dropped(PathBuf),
}

/// The native menu bar of [`MENU_BAR`]; built on the main thread, as `muda`
/// requires.
fn menu_bar() -> muda::Result<Menu> {
    let bar = Menu::new();
    for (name, items) in MENU_BAR {
        let built: Vec<Box<dyn IsMenuItem>> = items
            .iter()
            .map(|item| menu_item(*item))
            .collect::<muda::Result<_>>()?;
        let refs: Vec<&dyn IsMenuItem> = built.iter().map(AsRef::as_ref).collect();
        bar.append(&Submenu::with_items(name, true, &refs)?)?;
    }
    Ok(bar)
}

fn menu_item(item: MenuItem) -> muda::Result<Box<dyn IsMenuItem>> {
    Ok(match item {
        MenuItem::Own(action, label, key) => {
            let own = muda::MenuItem::with_id(action.id(), label, true, None);
            own.set_key_accelerator(Some(KeyAccelerator::new(
                CMD_OR_CTRL,
                Key::Character(key.to_string()),
            )))?;
            Box::new(own)
        }
        MenuItem::About => Box::new(PredefinedMenuItem::about(
            None,
            Some(AboutMetadata {
                name: Some("Signal".to_owned()),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                ..AboutMetadata::default()
            }),
        )),
        MenuItem::Hide => Box::new(PredefinedMenuItem::hide(None)),
        MenuItem::HideOthers => Box::new(PredefinedMenuItem::hide_others(None)),
        MenuItem::ShowAll => Box::new(PredefinedMenuItem::show_all(None)),
        MenuItem::Undo => Box::new(PredefinedMenuItem::undo(None)),
        MenuItem::Redo => Box::new(PredefinedMenuItem::redo(None)),
        MenuItem::Cut => Box::new(PredefinedMenuItem::cut(None)),
        MenuItem::Copy => Box::new(PredefinedMenuItem::copy(None)),
        MenuItem::Paste => Box::new(PredefinedMenuItem::paste(None)),
        MenuItem::SelectAll => Box::new(PredefinedMenuItem::select_all(None)),
        MenuItem::Separator => Box::new(PredefinedMenuItem::separator()),
    })
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
    let menu = match menu_bar() {
        Ok(menu) => menu,
        Err(error) => {
            eprintln!("signal-app: the menu bar was not built: {error}");
            return ExitCode::FAILURE;
        }
    };
    let menus = std::sync::Mutex::new(event_loop.create_proxy());
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some(action) = MenuAction::of(event.id().as_ref())
            && let Ok(proxy) = menus.lock()
        {
            let _ = proxy.send_event(UserEvent::Menu(action));
        }
    }));
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
    let drops = event_loop.create_proxy();
    let webview = WebViewBuilder::new()
        .with_url(url)
        // The page leaves the menu's shortcuts to the native menu.
        .with_initialization_script(NATIVE_MENU_SCRIPT)
        .with_navigation_handler(move |to| may_navigate(&to, &origin))
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
            // The application exists now: its menu bar is set.
            Event::NewEvents(StartCause::Init) => {
                menu.init_for_nsapp();
                false
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
            | Event::UserEvent(UserEvent::Menu(MenuAction::Quit)) => true,
            // Open Project…: the system's folder chooser; a chosen folder is
            // opened as a dropped one is, a cancel does nothing.
            Event::UserEvent(UserEvent::Menu(MenuAction::Open))
                if MainThreadMarker::new().is_some() =>
            {
                if let Some(path) = choose_folder() {
                    hand_dropped(&webview, &path);
                }
                false
            }
            Event::UserEvent(UserEvent::Menu(action)) => {
                if let Some(script) = action.page_script()
                    && let Err(error) = webview.evaluate_script(&script)
                {
                    eprintln!("signal-app: {error}");
                }
                false
            }
            Event::UserEvent(UserEvent::Dropped(path)) => {
                hand_dropped(&webview, &path);
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

/// Hands `path` to the page as a drop: a folder is opened; anything else is
/// refused, and opens nothing.
fn hand_dropped(webview: &wry::WebView, path: &std::path::Path) {
    let (event, text) = match dropped(path) {
        Ok(dir) => ("signal-drop", dir.display().to_string()),
        Err(refusal) => ("signal-app-error", refusal.to_owned()),
    };
    let quoted = serde_json::to_string(&text).unwrap_or_else(|_| "\"\"".to_owned());
    let script =
        format!("window.dispatchEvent(new CustomEvent('{event}', {{ detail: {quoted} }}))");
    if let Err(error) = webview.evaluate_script(&script) {
        eprintln!("signal-app: {error}");
    }
}

/// The system's chooser for a project (NSOpenPanel, modal): a folder, or a
/// project's `project.toml` (which opens its folder), on the main thread;
/// the choice, or None when cancelled.
fn choose_folder() -> Option<PathBuf> {
    let mtm = MainThreadMarker::new()?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(true);
    panel.setAllowsMultipleSelection(false);
    panel.setPrompt(Some(&NSString::from_str("Open")));
    panel.setMessage(Some(&NSString::from_str(
        "Choose a project folder, or its project.toml",
    )));
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    let path = panel.URL()?.path()?;
    Some(PathBuf::from(path.to_string()))
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
