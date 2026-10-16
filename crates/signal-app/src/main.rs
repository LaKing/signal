//! `signal-app`: Signal.app's window (ADR 0013, 0023), a tao window with a
//! wry WebView on the app's own UI server ([`AppServer`]). `--setup <file>`
//! and `--project <dir or .signal>` choose the Setup file and the project to
//! open, for tests and scripting.
//!
//! The menu bar is native (`muda`, M2.8-11): the app menu, File (Open…,
//! Save, Save As…, Interface Setup…, Quit) and Edit, so the page's text
//! fields get the system's shortcuts. Save and Interface Setup are handed
//! to the page, which runs them (a `signal-menu` event). Open… and Save As…
//! show the system's panels; a chosen, dropped or Finder-opened folder,
//! `project.toml` or `.signal` file is opened by the server (M2.8-12).
//! While a `.signal` project is edited, closing the window, Quit and opening
//! another project ask "Save changes to <name>?" (`window::prompt_buttons`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use muda::accelerator::{CMD_OR_CTRL, Key, KeyAccelerator, Modifiers};
use muda::{AboutMetadata, IsMenuItem, Menu, MenuEvent, PredefinedMenuItem, Submenu};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSModalResponseOK, NSOpenPanel, NSSavePanel,
};
use objc2_foundation::NSString;
use signal_app::server::{AppConfig, AppServer};
use signal_app::setup::{Setup, setup_path};
use signal_app::state::Backend;
use signal_app::window::{
    ExitPlan, MENU_BAR, MenuAction, MenuItem, NATIVE_MENU_SCRIPT, Prompt, QuitRoute,
    default_save_name, dropped, exit_plan, may_navigate, origin_of, prompt_buttons, prompt_text,
    save_target, title, window_of,
};
use signal_ui::document::open_root;
use signal_ui::{Edits, ProjectStatus};
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
    /// The project folder or .signal file to open (else the Setup's last
    /// project).
    #[arg(long)]
    project: Option<PathBuf>,
}

/// What the menu bar, the WebView and the server tell the event loop.
#[derive(Debug)]
enum UserEvent {
    /// One of Signal's own menu items.
    Menu(MenuAction),
    /// What was dropped on the window (a folder or `.signal` file is opened).
    Dropped(PathBuf),
    /// The open project or its edited state changed (the title).
    Project(Option<ProjectStatus>),
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
            let modifiers = if action.shift() {
                CMD_OR_CTRL | Modifiers::SHIFT
            } else {
                CMD_OR_CTRL
            };
            own.set_key_accelerator(Some(KeyAccelerator::new(
                modifiers,
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
    // Opened .signal files are unpacked under Application Support/Signal/Open
    // (beside the Setup's folder when HOME is not set).
    let open = open_root(std::env::var_os("HOME")).unwrap_or_else(|| path.with_file_name("Open"));
    // The bundle's ui/ (Contents/Resources/ui), else the repository's.
    let exe = std::env::current_exe().ok();
    let built = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui"));

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let projects = event_loop.create_proxy();
    let server = AppServer::start(AppConfig {
        setup_path: path,
        project,
        open_root: open,
        backend: Backend::CoreAudio,
        ui_dir: signal_ui::library_dir(exe.as_deref(), built),
        on_project: Some(Box::new(move |status| {
            let _ = projects.send_event(UserEvent::Project(status));
        })),
    });
    let server = match server {
        Ok(server) => server,
        Err(error) => {
            eprintln!("signal-app: {error}");
            return ExitCode::FAILURE;
        }
    };

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
            // The server opens what was dropped; nothing is navigated to.
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
        // What closes the app, and the prompt's route.
        let quit = match event {
            // The application exists now: its menu bar is set.
            Event::NewEvents(StartCause::Init) => {
                menu.init_for_nsapp();
                None
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => Some(QuitRoute::Window),
            Event::UserEvent(UserEvent::Menu(MenuAction::Quit)) => Some(QuitRoute::Menu),
            // The Dock's Quit or logout: termination is already decided.
            Event::LoopDestroyed => Some(QuitRoute::Terminate),
            // Open…: the system's chooser; a choice opens as a drop does, a
            // cancel does nothing.
            Event::UserEvent(UserEvent::Menu(MenuAction::Open))
                if MainThreadMarker::new().is_some() =>
            {
                if let (Some(path), Some(server)) = (choose_project(), server.as_ref()) {
                    open_project(server, &webview, &path);
                }
                None
            }
            Event::UserEvent(UserEvent::Menu(MenuAction::SaveAs)) => {
                if let Some(server) = server.as_ref() {
                    save_as(server, &webview);
                }
                None
            }
            Event::UserEvent(UserEvent::Menu(action)) => {
                if let Some(script) = action.page_script()
                    && let Err(error) = webview.evaluate_script(&script)
                {
                    eprintln!("signal-app: {error}");
                }
                None
            }
            Event::UserEvent(UserEvent::Dropped(path)) => {
                if let Some(server) = server.as_ref() {
                    open_project(server, &webview, &path);
                }
                None
            }
            // A .signal double-clicked in the Finder (or dropped on the Dock
            // icon): tao's open-file event.
            Event::Opened { urls } => {
                let path = urls.iter().find_map(|url| url.to_file_path().ok());
                if let (Some(path), Some(server)) = (path, server.as_ref()) {
                    open_project(server, &webview, &path);
                }
                None
            }
            Event::UserEvent(UserEvent::Project(status)) => {
                window.set_title(&title(status.as_ref()));
                None
            }
            _ => None,
        };
        let Some(route) = quit else {
            return;
        };
        let Some(running) = server.as_ref() else {
            return;
        };
        let discard = match closing(running, &webview, route) {
            Some(discard) => discard,
            None => return,
        };
        // The size the window has now, in points, goes into the Setup.
        let size = window.inner_size().to_logical::<f64>(window.scale_factor());
        if let Some(mut server) = server.take() {
            server.exit(window_of(size.width, size.height), discard);
        }
        *control_flow = ControlFlow::Exit;
    })
}

/// Before the app closes by `route`: an edited `.signal` project's prompt.
/// `None` stays (Cancel, or a failed save); else whether the edits are let
/// go (an unanswered prompt keeps them in the working folder).
fn closing(server: &AppServer, webview: &wry::WebView, route: QuitRoute) -> Option<bool> {
    let project = server.project();
    let edited = project.as_ref().is_some_and(|p| p.edited);
    let answer = match &project {
        Some(project) if edited => ask(route, &project.name),
        _ => None,
    };
    match exit_plan(edited, answer) {
        ExitPlan::Go | ExitPlan::Keep => Some(false),
        ExitPlan::Discard => Some(true),
        ExitPlan::Stay => None,
        ExitPlan::Save => match server.save() {
            Ok(()) => Some(false),
            // Terminating cannot stay: the working folder keeps the edits.
            Err(message) if route == QuitRoute::Terminate => {
                eprintln!("signal-app: {message}");
                Some(false)
            }
            Err(message) => {
                tell_page(webview, "signal-app-error", &message);
                None
            }
        },
    }
}

/// Opens what was chosen, dropped or opened in the Finder: refused unless a
/// folder, `project.toml` or `.signal` file; an edited project is asked
/// about first.
fn open_project(server: &AppServer, webview: &wry::WebView, path: &Path) {
    let target = match dropped(path) {
        Ok(target) => target,
        Err(refusal) => return tell_page(webview, "signal-app-error", refusal),
    };
    let project = server.project();
    let edits = match &project {
        Some(project) if project.edited => {
            match exit_plan(true, ask(QuitRoute::Open, &project.name)) {
                ExitPlan::Save => Edits::Save,
                ExitPlan::Discard => Edits::Discard,
                // Cancel, or no answer: nothing is opened.
                _ => return,
            }
        }
        _ => Edits::Ask,
    };
    if let Err(message) = server.open(target, edits) {
        tell_page(webview, "signal-app-error", &message);
    }
}

/// Save As…: the system's save panel with `<project name>.signal`; the
/// project is that file from then on.
fn save_as(server: &AppServer, webview: &wry::WebView) {
    let name = default_save_name(server.project().as_ref());
    let Some(file) = choose_save(&name) else {
        return;
    };
    let file = match save_target(file) {
        Ok(file) => file,
        Err(message) => return tell_page(webview, "signal-app-error", &message),
    };
    match server.save_as(&file) {
        Ok(()) => tell_page(
            webview,
            "signal-app-notice",
            &format!("saved as {}", file.display()),
        ),
        Err(message) => tell_page(webview, "signal-app-error", &message),
    }
}

/// Hands `text` to the page as the window event `event` (a notice or an
/// error, shown where the app shows its messages).
fn tell_page(webview: &wry::WebView, event: &str, text: &str) {
    let quoted = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned());
    let script =
        format!("window.dispatchEvent(new CustomEvent('{event}', {{ detail: {quoted} }}))");
    if let Err(error) = webview.evaluate_script(&script) {
        eprintln!("signal-app: {error}");
    }
}

/// "Save changes to <name>?" as a native alert (NSAlert, modal, safe API)
/// with the buttons of `route`; the answer, or None when it cannot run.
fn ask(route: QuitRoute, name: &str) -> Option<Prompt> {
    let mtm = MainThreadMarker::new()?;
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&prompt_text(name)));
    alert.setInformativeText(&NSString::from_str(
        "Your changes are lost if you don't save them.",
    ));
    for prompt in prompt_buttons(route) {
        alert.addButtonWithTitle(&NSString::from_str(prompt.label()));
    }
    let index = alert.runModal().checked_sub(NSAlertFirstButtonReturn)?;
    Prompt::of(route, usize::try_from(index).ok()?)
}

/// The system's chooser for a project (NSOpenPanel, modal): a folder, a
/// project's `project.toml` (which opens its folder) or a `.signal` file,
/// on the main thread; the choice, or None when cancelled.
fn choose_project() -> Option<PathBuf> {
    let mtm = MainThreadMarker::new()?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(true);
    panel.setAllowsMultipleSelection(false);
    panel.setPrompt(Some(&NSString::from_str("Open")));
    panel.setMessage(Some(&NSString::from_str(
        "Choose a project folder, its project.toml, or a .signal file",
    )));
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    let path = panel.URL()?.path()?;
    Some(PathBuf::from(path.to_string()))
}

/// The system's save panel (NSSavePanel, modal) named `name`; the chosen
/// file, or None when cancelled.
fn choose_save(name: &str) -> Option<PathBuf> {
    let mtm = MainThreadMarker::new()?;
    let panel = NSSavePanel::savePanel(mtm);
    panel.setNameFieldStringValue(&NSString::from_str(name));
    panel.setCanCreateDirectories(true);
    panel.setPrompt(Some(&NSString::from_str("Save")));
    panel.setMessage(Some(&NSString::from_str(
        "Save the project as one .signal file",
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
