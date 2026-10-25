//! `signal-app`: Signal.app's windows (ADR 0013, 0023, 0032), each a tao
//! window with a wry WebView on the app's own UI server ([`AppServer`]):
//! a main window (the app's page), a schematic window or a panel window,
//! any number of each, on any screen. `--setup <file>` and `--project <dir
//! or .signal>` choose the Setup file and the project to open, for tests
//! and scripting.
//!
//! The menu bar is native (`muda`, M2.8-11): the app menu (Settings…,
//! Quit), File (Open…, Save, Save As…, Revert to Saved…, Interface
//! Setup…, Quit), Edit, so
//! the page's text fields get the system's shortcuts, Project, Window (New
//! Main, Schematic and Panel Window, Reset Layout, Minimize, Zoom, full
//! screen and the window list) and Help (Keyboard Shortcuts). Each of
//! Signal's own items takes its chord from the keymap (ADR 0034: the
//! built-in `ui/lib/keys.default.js` and the person's `keys.toml` beside
//! the Setup), set again whenever the control thread says it changed; a
//! command's other chords come from the page (`menu.run`). Settings… and
//! Help ▸ Keyboard Shortcuts open the Settings window, which is no project
//! window (never in the layout; closing it closes only it). Save and Interface Setup are handed to a main window's page,
//! which runs them (a `signal-menu` event; without a main window one is
//! opened for them). Open… and Save As… show the system's panels; a
//! chosen, dropped or Finder-opened folder, `project.toml` or `.signal`
//! file is opened by the server (M2.8-12). While a `.signal` project is
//! edited, closing the last window, Quit and opening another project ask
//! "Save changes to <name>?" (`window::prompt_buttons`); closing any other
//! window just closes it. Revert to Saved…, enabled only while a `.signal`
//! project has changes (`window::can_revert`), asks "Revert to the last
//! saved version of <name>?" (Cancel, Revert) and then opens the file again
//! through the server, its changes let go.
//!
//! The windows' layout (ADR 0032) is the project's `ui/windows.toml`,
//! under the key of the connected screens (`layout::screen_key`): applied
//! at start, when another project opens and when the screens change
//! (looked at every second); without an entry for the key, the default
//! state (one main window on the main screen with every card). Moving,
//! resizing, opening or closing a window, or moving a card, writes the
//! key's entry a second after the last change (through the control
//! thread, which takes the write as seen). A panel card is moved from its
//! context menu: the page tells the app (`card.move`, `card.order`, wry's
//! IPC), which moves it and tells every window what it shows
//! (`signal-window`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use clap::Parser;
use muda::accelerator::KeyAccelerator;
use muda::{AboutMetadata, IsMenuItem, Menu, MenuEvent, PredefinedMenuItem, Submenu};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSModalResponseOK, NSOpenPanel, NSSavePanel,
};
use objc2_foundation::NSString;
use serde_json::{Value, json};
use signal_app::layout::{
    CardMove, Change, Layout, LayoutWindow, Monitor, Screens, Seen, Tracker, WindowView, centred,
    default_layout, fit_height, held, holder, labels, move_card, place, read_layout, record,
    reorder, screens_of, similar, write_layout,
};
use signal_app::server::{AppConfig, AppServer, WindowsEdit};
use signal_app::setup::{Setup, setup_path};
use signal_app::state::Backend;
use signal_app::window::{
    ExitPlan, HELP_MENU, MENU_BAR, MenuAction, MenuItem, NATIVE_MENU_SCRIPT, Prompt, QuitRoute,
    REVERT_BUTTONS, REVERT_INFORMATIVE, RevertPlan, RevertPrompt, SETTINGS_TITLE, WINDOW_MENU,
    can_revert, default_save_name, dropped, exit_plan, may_navigate, menu_chords, origin_of,
    page_url, prompt_buttons, prompt_text, revert_plan, revert_text, save_target, settings_path,
    view_path, view_title, window_of, window_title,
};
use signal_ui::document::open_root;
use signal_ui::keys::{Keys, keys_path};
use signal_ui::{Edits, ProjectStatus, WINDOWS_FILE};
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Fullscreen, WindowBuilder, WindowId};
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

/// What the menu bar, the WebViews and the server tell the event loop.
#[derive(Debug)]
enum UserEvent {
    /// One of Signal's own menu items.
    Menu(MenuAction),
    /// What was dropped on a window (a folder or `.signal` file is opened).
    Dropped(PathBuf),
    /// The open project or its edited state changed (the titles).
    Project(Option<ProjectStatus>),
    /// A window's page said something (wry's IPC: `window.ready`,
    /// `card.move`, `card.order`, `window.show`, `window.fit`, `menu.run`).
    Page(WindowId, String),
    /// The keymap changed (ADR 0034): each menu action's chord.
    Keys(Vec<(MenuAction, Option<String>)>),
}

type Target = EventLoopWindowTarget<UserEvent>;
/// A write's answer from the control thread.
type Answer = Receiver<Result<(), String>>;
type Proxy = EventLoopProxy<UserEvent>;

/// How often the screens are looked at (tao tells no screen change).
const SCREEN_POLL: Duration = Duration::from_secs(1);
/// New Panel Window's size, in points.
const PANEL_SIZE: (f64, f64) = (600.0, 400.0);
/// New Schematic Window's size, in points.
const SCHEMATIC_SIZE: (f64, f64) = (1200.0, 800.0);
/// The Settings window's size, in points.
const SETTINGS_SIZE: (f64, f64) = (760.0, 640.0);

/// The menu bar as built: the bar, its Window and Help menus, and Signal's
/// own items, whose chords follow the keymap.
struct MenuBar {
    bar: Menu,
    windows: Option<Submenu>,
    help: Option<Submenu>,
    items: Vec<(MenuAction, muda::MenuItem)>,
}

impl MenuBar {
    /// Gives each of Signal's own items its chord (ADR 0034); a chord
    /// `muda` does not take is reported and left off.
    fn set_chords(&self, chords: &[(MenuAction, Option<String>)]) {
        for (action, item) in &self.items {
            let chord = chords
                .iter()
                .find(|(a, _)| a == action)
                .and_then(|(_, c)| c.as_deref());
            let accelerator = chord.and_then(|text| match text.parse::<KeyAccelerator>() {
                Ok(accelerator) => Some(accelerator),
                Err(error) => {
                    eprintln!(
                        "signal-app: the menu cannot show {text} for {}: {error}",
                        action.command()
                    );
                    None
                }
            });
            if let Err(error) = item.set_key_accelerator(accelerator) {
                eprintln!("signal-app: {}: {error}", action.command());
            }
        }
    }

    /// Enables Revert to Saved… while `project` has changes to let go
    /// (`window::can_revert`).
    fn set_project(&self, project: Option<&ProjectStatus>) {
        let enabled = can_revert(project);
        for (action, item) in &self.items {
            if *action == MenuAction::Revert {
                item.set_enabled(enabled);
            }
        }
    }
}

/// The native menu bar of [`MENU_BAR`], its Window and Help menus, its
/// items' chords the keymap's (`chords`); built on the main thread, as
/// `muda` requires.
fn menu_bar(chords: &[(MenuAction, Option<String>)]) -> muda::Result<MenuBar> {
    let bar = Menu::new();
    let mut windows = None;
    let mut help = None;
    let mut own = Vec::new();
    for (name, items) in MENU_BAR {
        let built: Vec<Box<dyn IsMenuItem>> = items
            .iter()
            .map(|item| menu_item(*item, &mut own))
            .collect::<muda::Result<_>>()?;
        let refs: Vec<&dyn IsMenuItem> = built.iter().map(AsRef::as_ref).collect();
        let submenu = Submenu::with_items(name, true, &refs)?;
        bar.append(&submenu)?;
        if name == WINDOW_MENU {
            windows = Some(submenu);
        } else if name == HELP_MENU {
            help = Some(submenu);
        }
    }
    let menu = MenuBar {
        bar,
        windows,
        help,
        items: own,
    };
    menu.set_chords(chords);
    Ok(menu)
}

fn menu_item(
    item: MenuItem,
    own: &mut Vec<(MenuAction, muda::MenuItem)>,
) -> muda::Result<Box<dyn IsMenuItem>> {
    Ok(match item {
        MenuItem::Own(action, label) => {
            let made = muda::MenuItem::with_id(action.id(), label, true, None);
            own.push((action, made.clone()));
            Box::new(made)
        }
        MenuItem::About => Box::new(PredefinedMenuItem::about(
            None,
            Some(AboutMetadata {
                name: Some("Signal".to_owned()),
                version: Some(signal_app::VERSION.to_owned()),
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
        MenuItem::Minimize => Box::new(PredefinedMenuItem::minimize(None)),
        MenuItem::Zoom => Box::new(PredefinedMenuItem::zoom(None)),
        MenuItem::Fullscreen => Box::new(PredefinedMenuItem::fullscreen(None)),
        MenuItem::BringAllToFront => Box::new(PredefinedMenuItem::bring_all_to_front(None)),
        MenuItem::Separator => Box::new(PredefinedMenuItem::separator()),
    })
}

/// The connected screens as tao lists them.
fn screens(target: &Target) -> Screens {
    let handles: Vec<_> = target.available_monitors().collect();
    let primary = target.primary_monitor();
    let monitors: Vec<Monitor> = handles
        .iter()
        .map(|m| Monitor {
            name: m.name().unwrap_or_else(|| "screen".to_owned()),
            width: m.size().width,
            height: m.size().height,
            scale: m.scale_factor(),
            x: m.position().x,
            y: m.position().y,
        })
        .collect();
    let primary = primary.and_then(|p| handles.iter().position(|m| *m == p));
    screens_of(&monitors, primary)
}

/// The script that dispatches window event `event` with `detail` (JSON).
fn event_script(event: &str, detail: &Value) -> String {
    format!("window.dispatchEvent(new CustomEvent('{event}', {{ detail: {detail} }}))")
}

/// One open window.
struct Shown {
    view: WindowView,
    /// A panel window's cards; a main window's card order.
    cards: Vec<String>,
    // Before `window`: fields drop in order, so the WebView goes first.
    webview: wry::WebView,
    window: tao::window::Window,
    /// Its page said `window.ready` (it listens for `signal-window`).
    ready: bool,
    /// Scripts to run once it is ready (a menu action for a main window
    /// opened for it).
    pending: Vec<String>,
    /// The person resized it: its size is theirs, never fitted.
    sized: bool,
    /// Its cards changed: its page's next `window.fit` sizes it.
    fit: bool,
    /// Until then, its resizes are the app's own (a fit).
    own_until: Instant,
}

impl Shown {
    /// Its frame's corner and content's size, in the desktop's points.
    fn frame(&self) -> ((f64, f64), (f64, f64)) {
        let scale = self.window.scale_factor();
        let at = self
            .window
            .outer_position()
            .map(|p| p.to_logical::<f64>(scale))
            .map_or((0.0, 0.0), |p| (p.x, p.y));
        let size = self.window.inner_size().to_logical::<f64>(scale);
        (at, (size.width, size.height))
    }

    fn fullscreen(&self) -> bool {
        self.window.fullscreen().is_some()
    }

    /// Full screen (natively, on its screen) or not, as `want` says.
    fn set_fullscreen(&self, want: bool) {
        if want != self.fullscreen() {
            self.window
                .set_fullscreen(want.then_some(Fullscreen::Borderless(None)));
        }
    }

    fn run(&self, script: &str) {
        if let Err(error) = self.webview.evaluate_script(script) {
            eprintln!("signal-app: {error}");
        }
    }
}

/// The open windows and their layout (ADR 0032).
struct Windows {
    list: Vec<Shown>,
    /// The windows, the most recently focused first.
    front: Vec<WindowId>,
    screens: Screens,
    polled: Instant,
    /// The project whose `ui/windows.toml` holds the layout.
    dir: Option<PathBuf>,
    project: Option<ProjectStatus>,
    app_url: String,
    origin: String,
    /// The Setup's `[window]`: the default main window's size.
    size: Option<(f64, f64)>,
    /// When to write (the screens' key, the person's changes, the guard).
    tracker: Tracker,
    /// The layout as last read or written for the key.
    written: Option<Layout>,
    /// A write under way: its answer, the layout written and when it was
    /// asked.
    writing: Option<(Answer, Layout, Instant)>,
    /// The Settings window (ADR 0034), when open: no project window, never
    /// in the layout.
    settings: Option<Settings>,
}

/// The Settings window: its WebView and window (dropped in this order).
struct Settings {
    webview: wry::WebView,
    window: tao::window::Window,
}

impl Windows {
    /// Opens a window of `view` holding `cards` at `at`.
    fn open(
        &mut self,
        target: &Target,
        proxy: &Proxy,
        view: WindowView,
        cards: Vec<String>,
        at: &LayoutWindow,
    ) -> Result<usize, String> {
        let placed = place(at, &self.screens);
        let window = WindowBuilder::new()
            .with_title(view_title(view, self.project.as_ref()))
            .with_inner_size(LogicalSize::new(placed.width, placed.height))
            .with_position(LogicalPosition::new(placed.x, placed.y))
            .build(target)
            .map_err(|error| format!("the window did not open: {error}"))?;
        let id = window.id();
        let origin = self.origin.clone();
        let drops = proxy.clone();
        let pages = proxy.clone();
        let webview = WebViewBuilder::new()
            .with_url(page_url(&self.app_url, view_path(view)))
            // The page leaves the menu's shortcuts to the native menu.
            .with_initialization_script(NATIVE_MENU_SCRIPT)
            .with_navigation_handler(move |to| may_navigate(&to, &origin))
            .with_ipc_handler(move |request: wry::http::Request<String>| {
                let _ = pages.send_event(UserEvent::Page(id, request.into_body()));
            })
            .with_drag_drop_handler(move |event| {
                if let DragDropEvent::Drop { paths, .. } = event
                    && let Some(path) = paths.into_iter().next()
                {
                    let _ = drops.send_event(UserEvent::Dropped(path));
                }
                // The server opens what was dropped; nothing is navigated to.
                true
            })
            .build(&window)
            .map_err(|error| format!("the WebView did not open: {error}"))?;
        self.list.push(Shown {
            view,
            cards,
            window,
            webview,
            ready: false,
            pending: Vec::new(),
            sized: false,
            fit: false,
            own_until: Instant::now(),
        });
        self.front.insert(0, id);
        if at.fullscreen
            && let Some(shown) = self.list.last()
        {
            shown.set_fullscreen(true);
        }
        self.tracker.change(Instant::now(), Change::Structure);
        Ok(self.list.len() - 1)
    }

    /// Whether `id` is the Settings window's.
    fn is_settings(&self, id: WindowId) -> bool {
        self.settings.as_ref().is_some_and(|s| s.window.id() == id)
    }

    /// Shows the Settings window (ADR 0034), opening it on the front
    /// window's screen; `reference`: its shortcuts as a reference.
    fn open_settings(&mut self, target: &Target, proxy: &Proxy, reference: bool) {
        if let Some(settings) = &self.settings {
            let view = if reference { "reference" } else { "edit" };
            let script = event_script("signal-settings", &json!({ "view": view }));
            if let Err(error) = settings.webview.evaluate_script(&script) {
                eprintln!("signal-app: {error}");
            }
            settings.window.set_minimized(false);
            settings.window.set_focus();
            return;
        }
        let screen = self.front_screen();
        let at = centred(WindowView::Panel, SETTINGS_SIZE, screen, &self.screens);
        let placed = place(&at, &self.screens);
        let window = match WindowBuilder::new()
            .with_title(SETTINGS_TITLE)
            .with_inner_size(LogicalSize::new(SETTINGS_SIZE.0, SETTINGS_SIZE.1))
            .with_position(LogicalPosition::new(placed.x, placed.y))
            .build(target)
        {
            Ok(window) => window,
            Err(error) => {
                eprintln!("signal-app: the Settings window did not open: {error}");
                return;
            }
        };
        let id = window.id();
        let origin = self.origin.clone();
        let pages = proxy.clone();
        let webview = WebViewBuilder::new()
            .with_url(page_url(&self.app_url, settings_path(reference)))
            .with_initialization_script(NATIVE_MENU_SCRIPT)
            .with_navigation_handler(move |to| may_navigate(&to, &origin))
            .with_ipc_handler(move |request: wry::http::Request<String>| {
                let _ = pages.send_event(UserEvent::Page(id, request.into_body()));
            })
            .build(&window);
        match webview {
            Ok(webview) => self.settings = Some(Settings { webview, window }),
            Err(error) => eprintln!("signal-app: the Settings page did not open: {error}"),
        }
    }

    /// A new window of `view` near the front window, on its screen.
    fn open_new(&mut self, target: &Target, proxy: &Proxy, view: WindowView) -> Option<usize> {
        let at = match view {
            WindowView::Main => default_layout(&self.screens, self.size).windows.remove(0),
            // On the front window's screen, in the middle.
            _ => {
                let size = if view == WindowView::Panel {
                    PANEL_SIZE
                } else {
                    SCHEMATIC_SIZE
                };
                let screen = self.front_screen();
                centred(view, size, screen, &self.screens)
            }
        };
        match self.open(target, proxy, view, Vec::new(), &at) {
            Ok(index) => Some(index),
            Err(error) => {
                eprintln!("signal-app: {error}");
                None
            }
        }
    }

    /// The screen of the front window (the main screen without one).
    fn front_screen(&self) -> usize {
        self.index(self.front.first().copied())
            .and_then(|i| self.layout().windows.get(i).map(|w| w.screen))
            .unwrap_or(self.screens.primary)
    }

    fn index(&self, id: Option<WindowId>) -> Option<usize> {
        id.and_then(|id| self.list.iter().position(|w| w.window.id() == id))
    }

    /// The layout the windows have now.
    fn layout(&self) -> Layout {
        Layout {
            windows: self
                .list
                .iter()
                .map(|shown| {
                    let (at, size) = shown.frame();
                    let cards = match shown.view {
                        WindowView::Schematic => Vec::new(),
                        _ => shown.cards.clone(),
                    };
                    LayoutWindow {
                        fullscreen: shown.fullscreen(),
                        ..record(shown.view, cards, at, size, &self.screens)
                    }
                })
                .collect(),
        }
    }

    /// The layout of the project for the screens now: its entry for their
    /// key, else the default state.
    fn stored(&self) -> Layout {
        let text = self
            .dir
            .as_ref()
            .and_then(|dir| std::fs::read_to_string(dir.join(WINDOWS_FILE)).ok())
            .unwrap_or_default();
        match read_layout(&text, &self.screens.key) {
            Ok(Some(layout)) if !layout.windows.is_empty() => layout,
            Ok(_) => default_layout(&self.screens, self.size),
            Err(problem) => {
                eprintln!("signal-app: {WINDOWS_FILE}: {problem}; the default layout");
                default_layout(&self.screens, self.size)
            }
        }
    }

    /// Shows `layout`: each of its windows reuses an open window of the
    /// same view (its page kept), moved there, or opens; the others close.
    fn apply(&mut self, target: &Target, proxy: &Proxy, layout: &Layout) {
        let mut old: Vec<Option<Shown>> = std::mem::take(&mut self.list)
            .into_iter()
            .map(Some)
            .collect();
        let mut kept: Vec<Shown> = Vec::new();
        let mut opened: Vec<&LayoutWindow> = Vec::new();
        for want in &layout.windows {
            let reuse = old
                .iter_mut()
                .find(|w| w.as_ref().is_some_and(|w| w.view == want.view))
                .and_then(Option::take);
            match reuse {
                Some(mut shown) => {
                    shown.set_fullscreen(false);
                    let placed = place(want, &self.screens);
                    shown
                        .window
                        .set_outer_position(LogicalPosition::new(placed.x, placed.y));
                    shown
                        .window
                        .set_inner_size(LogicalSize::new(placed.width, placed.height));
                    shown.cards = want.cards.clone();
                    if want.fullscreen {
                        shown.set_fullscreen(true);
                    }
                    kept.push(shown);
                }
                None => opened.push(want),
            }
        }
        // What is left closes (dropped).
        drop(old);
        self.list = kept;
        for want in opened {
            if let Err(error) = self.open(target, proxy, want.view, want.cards.clone(), want) {
                eprintln!("signal-app: {error}");
            }
        }
        let open: Vec<WindowId> = self.list.iter().map(|w| w.window.id()).collect();
        self.front.retain(|id| open.contains(id));
        self.written = Some(layout.clone());
        self.tracker.applied(Instant::now());
        self.tell_cards();
    }

    /// Tells every ready page which cards it shows.
    fn tell_cards(&self) {
        let layout = self.layout();
        let names = labels(&layout);
        for (index, shown) in self.list.iter().enumerate() {
            shown.window.set_title(&window_title(
                shown.view,
                names.get(index).map_or("", String::as_str),
                self.project.as_ref(),
            ));
            if shown.ready
                && let Some(script) = Self::cards_script(index, &layout, &names)
            {
                shown.run(&script);
            }
        }
    }

    /// What window `index` of `layout` shows (`signal-window`): its cards,
    /// a main window's held ones with where they are, and the windows its
    /// cards may move to.
    fn cards_script(index: usize, layout: &Layout, names: &[String]) -> Option<String> {
        let window = layout.windows.get(index)?;
        let targets: Vec<Value> = layout
            .windows
            .iter()
            .enumerate()
            .filter(|(i, w)| *i != index && w.view != WindowView::Schematic)
            .map(|(i, _)| json!({"index": i, "label": names.get(i)}))
            .collect();
        let detail = match window.view {
            WindowView::Schematic => return None,
            WindowView::Panel => {
                json!({"view": "panel", "cards": window.cards, "targets": targets})
            }
            WindowView::Main => {
                let held: Vec<Value> = held(layout)
                    .into_iter()
                    .map(|card| {
                        let at = holder(layout, &card).and_then(|i| names.get(i));
                        json!({"card": card, "where": at})
                    })
                    .collect();
                json!({"view": "main", "held": held, "order": window.cards, "targets": targets})
            }
        };
        Some(event_script("signal-window", &detail))
    }

    /// Runs `script` in a main window's page (the front one), opening one
    /// for it when there is none.
    fn in_main(&mut self, target: &Target, proxy: &Proxy, script: String) {
        let front = self
            .front
            .iter()
            .filter_map(|id| self.index(Some(*id)))
            .find(|&i| self.list[i].view == WindowView::Main)
            .or_else(|| self.list.iter().position(|w| w.view == WindowView::Main));
        let index = match front {
            Some(index) => Some(index),
            None => self.open_new(target, proxy, WindowView::Main),
        };
        if let Some(shown) = index.and_then(|i| self.list.get_mut(i)) {
            if shown.ready {
                shown.run(&script);
            } else {
                shown.pending.push(script);
            }
        }
    }

    /// Hands `text` to a main window's page as the window event `event` (a
    /// notice or an error, shown where the app shows its messages).
    fn tell(&mut self, target: &Target, proxy: &Proxy, event: &str, text: &str) {
        self.in_main(target, proxy, event_script(event, &json!(text)));
    }

    /// The edit of `ui/windows.toml` that writes `layout` for the key now:
    /// its entry, removed when it is the default state.
    fn edit_for(&self, layout: &Layout) -> WindowsEdit {
        let key = self.screens.key.clone();
        let entry =
            (!similar(layout, &default_layout(&self.screens, self.size))).then(|| layout.clone());
        Box::new(move |text: &str| write_layout(text, &key, entry.as_ref()))
    }

    /// Writes the layout when the person changed it and it settled
    /// (`Tracker::due`), without waiting for the answer, which
    /// [`Self::answered`] takes; `force` (Quit, opening another project)
    /// does not wait for it to settle, and waits for the write.
    fn save(&mut self, server: &AppServer, force: bool) {
        let now = Instant::now();
        if force && let Some((answer, layout, asked)) = self.writing.take() {
            let answer = answer.recv_timeout(Duration::from_secs(5));
            self.answer(answer.ok(), layout, asked);
        }
        if self.writing.is_some() || !self.tracker.due(now, force) {
            return;
        }
        let layout = self.layout();
        let Some(dir) = self.dir.clone() else {
            self.tracker.written(now);
            return;
        };
        if self.written.as_ref().is_some_and(|w| similar(w, &layout)) {
            self.tracker.written(now);
            return;
        }
        let edit = self.edit_for(&layout);
        if force {
            let answer = server.write_windows(&dir, edit);
            self.answer(Some(answer), layout, now);
        } else {
            self.writing = Some((server.write_windows_later(&dir, edit), layout, now));
        }
    }

    /// A write's answer, if it came.
    fn answered(&mut self) {
        let Some((answer, ..)) = &self.writing else {
            return;
        };
        let answer = match answer.try_recv() {
            Ok(answer) => Some(answer),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        };
        if let Some((_, layout, asked)) = self.writing.take() {
            self.answer(answer, layout, asked);
        }
    }

    /// What a write answered: written, or tried again once settled anew
    /// (the failure goes to the log).
    fn answer(&mut self, answer: Option<Result<(), String>>, layout: Layout, asked: Instant) {
        match answer {
            Some(Ok(())) => {
                self.written = Some(layout);
                self.tracker.written(asked);
            }
            Some(Err(message)) => {
                eprintln!("signal-app: {message}");
                self.tracker.failed(asked, Instant::now());
            }
            None => {
                eprintln!("signal-app: {WINDOWS_FILE} was not written: no answer");
                self.tracker.failed(asked, Instant::now());
            }
        }
    }

    /// Another project was opened from a page (the menu writes first): the
    /// layout's pending change goes into the project it was of, directly,
    /// since the control thread serves the new one now: a folder project,
    /// whose folder stays (a `.signal` project's working folder closed with
    /// its document).
    fn flush_to(&mut self, old: Option<&ProjectStatus>) {
        let now = Instant::now();
        let Some(old) = old.filter(|p| p.file.is_none() && p.dir.is_dir()) else {
            return;
        };
        if !self.tracker.due(now, true) {
            return;
        }
        let layout = self.layout();
        if self.written.as_ref().is_some_and(|w| similar(w, &layout)) {
            return;
        }
        let path = old.dir.join(WINDOWS_FILE);
        // Read, edited and written under the project's writer lock (ADR 0040).
        let lock = signal_project::transaction::ProjectLock::acquire(&old.dir);
        if let Err(error) = &lock {
            eprintln!("signal-app: {} was not written: {error}", path.display());
            return;
        }
        // Edited again if it changes before the rename.
        let edit = self.edit_for(&layout);
        let written = signal_project::transaction::edit_text(&path, |text| {
            edit(text).map(|after| (after != text).then_some(after))
        });
        if let Err(message) = written {
            eprintln!("signal-app: {} was not written: {message}", path.display());
        }
        self.tracker.written(now);
    }

    /// Looks at the screens (once a second): another configuration, once
    /// stable (`Tracker::screens`), shows its layout; meanwhile nothing is
    /// applied or written.
    fn poll(&mut self, target: &Target, proxy: &Proxy) {
        if self.polled.elapsed() < SCREEN_POLL {
            return;
        }
        let now = Instant::now();
        self.polled = now;
        let seen = screens(target);
        let key = (!seen.screens.is_empty()).then_some(seen.key.as_str());
        match self.tracker.screens(now, key) {
            Seen::Same => self.screens = seen,
            Seen::Waiting => {}
            Seen::Switch => {
                self.screens = seen;
                let layout = self.stored();
                self.apply(target, proxy, &layout);
            }
        }
    }

    /// The window's page said `text`.
    fn page(&mut self, target: &Target, proxy: &Proxy, id: WindowId, text: &str) {
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            return;
        };
        // ADR 0034: a menu command's other chord, pressed in any window
        // (the native menu takes the first): as if its item were chosen.
        if message.get("type").and_then(Value::as_str) == Some("menu.run") {
            if let Some(action) = message
                .get("command")
                .and_then(Value::as_str)
                .and_then(MenuAction::of_command)
            {
                let _ = proxy.send_event(UserEvent::Menu(action));
            }
            return;
        }
        let Some(index) = self.index(Some(id)) else {
            return;
        };
        let card = message
            .get("card")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let size = message.get("size").and_then(|size| {
            let w = size.get("width")?.as_f64()?;
            let h = size.get("height")?.as_f64()?;
            (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some((w, h))
        });
        match (message.get("type").and_then(Value::as_str), card) {
            (Some("window.ready"), _) => {
                let layout = self.layout();
                let script = Self::cards_script(index, &layout, &labels(&layout));
                let shown = &mut self.list[index];
                shown.ready = true;
                if let Some(script) = script {
                    shown.run(&script);
                }
                for script in std::mem::take(&mut shown.pending) {
                    shown.run(&script);
                }
            }
            (Some("card.order"), _) => {
                let order: Vec<String> = message
                    .get("cards")
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let mut layout = self.layout();
                if reorder(&mut layout, index, &order) {
                    self.cards_from(&layout);
                }
            }
            (Some("card.move"), Some(card)) => {
                let to = message.get("to");
                match to.and_then(Value::as_str) {
                    Some("here") => self.move_to(&card, index),
                    Some("main") => {
                        let main = self
                            .front_index(WindowView::Main)
                            .or_else(|| self.open_new(target, proxy, WindowView::Main));
                        if let Some(main) = main {
                            self.move_to(&card, main);
                        }
                    }
                    Some("new") => {
                        let screen = self.layout().windows.get(index).map_or(0, |w| w.screen);
                        let size = size.unwrap_or(PANEL_SIZE);
                        let window = centred(WindowView::Panel, size, screen, &self.screens);
                        self.new_window_with(target, proxy, &card, window);
                    }
                    _ => {
                        if let Some(to) = to
                            .and_then(Value::as_u64)
                            .and_then(|i| usize::try_from(i).ok())
                        {
                            self.move_to(&card, to);
                        }
                    }
                }
            }
            (Some("window.show"), Some(card)) => {
                if let Some(at) = holder(&self.layout(), &card)
                    && let Some(shown) = self.list.get(at)
                {
                    shown.window.set_minimized(false);
                    shown.window.set_focus();
                }
            }
            (Some("window.fit"), _) => {
                let height = message.get("height").and_then(Value::as_f64);
                self.fit(index, height);
            }
            _ => {}
        }
    }

    /// Moves `card` into window `to`, at its end.
    fn move_to(&mut self, card: &str, to: usize) {
        let mut layout = self.layout();
        if move_card(&mut layout, card, CardMove::Into(to)) {
            self.cards_from(&layout);
        }
    }

    /// The front window of `view`, else the first.
    fn front_index(&self, view: WindowView) -> Option<usize> {
        self.front
            .iter()
            .filter_map(|id| self.index(Some(*id)))
            .find(|&i| self.list[i].view == view)
            .or_else(|| self.list.iter().position(|w| w.view == view))
    }

    /// Opens a panel window laid out as `window` holding `card`.
    fn new_window_with(
        &mut self,
        target: &Target,
        proxy: &Proxy,
        card: &str,
        window: LayoutWindow,
    ) {
        let mut layout = self.layout();
        if move_card(&mut layout, card, CardMove::New(window.clone())) {
            self.cards_from(&layout);
            if let Err(error) = self.open(
                target,
                proxy,
                WindowView::Panel,
                vec![card.to_owned()],
                &window,
            ) {
                eprintln!("signal-app: {error}");
            }
            self.tell_cards();
        }
    }

    /// Window `index`'s page measured its content `height`: a panel window
    /// whose cards changed, and that the person has not resized, fits it.
    fn fit(&mut self, index: usize, height: Option<f64>) {
        let layout = self.layout();
        let (Some(height), Some(window), Some(shown)) =
            (height, layout.windows.get(index), self.list.get_mut(index))
        else {
            return;
        };
        if !shown.fit || shown.sized || shown.view != WindowView::Panel || shown.fullscreen() {
            return;
        }
        shown.fit = false;
        let height = fit_height(window, height, &self.screens);
        if (height - window.height).abs() >= 2.0 {
            shown.own_until = Instant::now() + Duration::from_secs(1);
            shown
                .window
                .set_inner_size(LogicalSize::new(window.width, height));
        }
    }

    /// Takes the cards of `layout`'s windows (by index; a new window at its
    /// end is opened by the caller) and tells the pages.
    fn cards_from(&mut self, layout: &Layout) {
        for (shown, window) in self.list.iter_mut().zip(&layout.windows) {
            if shown.cards != window.cards {
                shown.fit = true;
            }
            shown.cards = window.cards.clone();
        }
        self.tracker.change(Instant::now(), Change::Structure);
        self.tell_cards();
    }

    /// The titles, after the project or its edited state changed.
    fn titles(&self) {
        let names = labels(&self.layout());
        for (shown, name) in self.list.iter().zip(&names) {
            shown
                .window
                .set_title(&window_title(shown.view, name, self.project.as_ref()));
        }
    }

    /// The default main window's size for the Setup: the first main
    /// window's, in points.
    fn main_size(&self) -> Option<signal_app::setup::Window> {
        let shown = self.list.iter().find(|w| w.view == WindowView::Main)?;
        let (_, (width, height)) = shown.frame();
        window_of(width, height)
    }
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
    // Read here for the main window's size and to report problems on the
    // terminal too; the control thread reads it again as its own.
    let (setup, problems) = Setup::load(&path);
    for problem in &problems {
        eprintln!("signal-app: {problem}");
    }
    // ADR 0034: the menu's chords at start (the control thread reports the
    // file's problems and tells of every change).
    let keys = Keys::load(&keys_path(&path));
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
    let keymaps = event_loop.create_proxy();
    let server = AppServer::start(AppConfig {
        setup_path: path,
        project,
        open_root: open,
        backend: Backend::CoreAudio,
        ui_dir: signal_ui::library_dir(exe.as_deref(), built),
        on_project: Some(Box::new(move |status| {
            let _ = projects.send_event(UserEvent::Project(status));
        })),
        on_keys: Some(Box::new(move |keys: &Keys| {
            let _ = keymaps.send_event(UserEvent::Keys(menu_chords(keys)));
        })),
    });
    let server = match server {
        Ok(server) => server,
        Err(error) => {
            eprintln!("signal-app: {error}");
            return ExitCode::FAILURE;
        }
    };

    let menu = match menu_bar(&menu_chords(&keys)) {
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

    let proxy = event_loop.create_proxy();
    let url = server.url().to_owned();
    let project = server.project();
    menu.set_project(project.as_ref());
    let mut windows = Windows {
        list: Vec::new(),
        front: Vec::new(),
        screens: screens(&event_loop),
        polled: Instant::now(),
        tracker: Tracker::new("", Instant::now()),
        dir: project.as_ref().map(|p| p.dir.clone()),
        project,
        origin: origin_of(&url).to_owned(),
        app_url: url,
        size: setup
            .window
            .map(|w| (f64::from(w.width), f64::from(w.height))),
        written: None,
        writing: None,
        settings: None,
    };
    windows.tracker = Tracker::new(&windows.screens.key, Instant::now());
    let layout = windows.stored();
    windows.apply(&event_loop, &proxy, &layout);
    if windows.list.is_empty() {
        eprintln!("signal-app: no window opened");
        return ExitCode::FAILURE;
    }

    let mut server = Some(server);
    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250));
        // What closes the app, and the prompt's route.
        let quit = match event {
            // The application exists now: its menu bar is set.
            Event::NewEvents(StartCause::Init) => {
                menu.bar.init_for_nsapp();
                if let Some(window_menu) = &menu.windows {
                    window_menu.set_as_windows_menu_for_nsapp();
                }
                if let Some(help) = &menu.help {
                    help.set_as_help_menu_for_nsapp();
                }
                None
            }
            // ADR 0034: the keymap changed; the menu's chords follow it.
            Event::UserEvent(UserEvent::Keys(chords)) => {
                menu.set_chords(&chords);
                None
            }
            Event::MainEventsCleared => {
                windows.poll(target, &proxy);
                windows.answered();
                if let Some(server) = server.as_ref() {
                    windows.save(server, false);
                }
                None
            }
            Event::WindowEvent {
                window_id, event, ..
            } => match event {
                // The Settings window is no project window: closing it
                // closes it, whatever else is open.
                WindowEvent::CloseRequested if windows.is_settings(window_id) => {
                    windows.settings = None;
                    None
                }
                _ if windows.is_settings(window_id) => None,
                // Closing a window just closes it; closing the last one
                // quits, as Signal's one window did.
                WindowEvent::CloseRequested if windows.list.len() > 1 => {
                    windows.list.retain(|w| w.window.id() != window_id);
                    windows.front.retain(|id| *id != window_id);
                    windows.tracker.change(Instant::now(), Change::Structure);
                    windows.tell_cards();
                    None
                }
                WindowEvent::CloseRequested => Some(QuitRoute::Window),
                WindowEvent::Focused(true) => {
                    windows.front.retain(|id| *id != window_id);
                    windows.front.insert(0, window_id);
                    None
                }
                WindowEvent::Resized(_) => {
                    let now = Instant::now();
                    // The person's resize (not the system's, not a fit):
                    // that window's size is theirs from now on.
                    if windows.tracker.change(now, Change::Geometry)
                        && let Some(i) = windows.index(Some(window_id))
                        && now >= windows.list[i].own_until
                    {
                        windows.list[i].sized = true;
                    }
                    None
                }
                WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    windows.tracker.change(Instant::now(), Change::Geometry);
                    None
                }
                _ => None,
            },
            Event::UserEvent(UserEvent::Menu(MenuAction::Quit)) => Some(QuitRoute::Menu),
            // The Dock's Quit or logout: termination is already decided.
            Event::LoopDestroyed => Some(QuitRoute::Terminate),
            Event::UserEvent(UserEvent::Menu(MenuAction::NewMain)) => {
                windows.open_new(target, &proxy, WindowView::Main);
                None
            }
            Event::UserEvent(UserEvent::Menu(MenuAction::NewSchematic)) => {
                windows.open_new(target, &proxy, WindowView::Schematic);
                None
            }
            Event::UserEvent(UserEvent::Menu(MenuAction::NewPanel)) => {
                windows.open_new(target, &proxy, WindowView::Panel);
                None
            }
            // ADR 0034: Settings… and Help ▸ Keyboard Shortcuts.
            Event::UserEvent(UserEvent::Menu(MenuAction::Settings)) => {
                windows.open_settings(target, &proxy, false);
                None
            }
            Event::UserEvent(UserEvent::Menu(MenuAction::KeysHelp)) => {
                windows.open_settings(target, &proxy, true);
                None
            }
            // Everything back on one main window on the main screen; the
            // key's entry is removed.
            Event::UserEvent(UserEvent::Menu(MenuAction::ResetLayout)) => {
                let layout = default_layout(&windows.screens, windows.size);
                windows.apply(target, &proxy, &layout);
                if let (Some(server), Some(dir)) = (server.as_ref(), windows.dir.clone()) {
                    let key = windows.screens.key.clone();
                    let edit = Box::new(move |text: &str| write_layout(text, &key, None));
                    if let Err(message) = server.write_windows(&dir, edit) {
                        eprintln!("signal-app: {message}");
                    }
                }
                None
            }
            // Open…: the system's chooser; a choice opens as a drop does, a
            // cancel does nothing.
            Event::UserEvent(UserEvent::Menu(MenuAction::Open))
                if MainThreadMarker::new().is_some() =>
            {
                if let (Some(path), Some(server)) = (choose_project(), server.as_ref()) {
                    open_project(server, &mut windows, target, &proxy, &path);
                }
                None
            }
            Event::UserEvent(UserEvent::Menu(MenuAction::SaveAs)) => {
                if let Some(server) = server.as_ref() {
                    save_as(server, &mut windows, target, &proxy);
                }
                None
            }
            Event::UserEvent(UserEvent::Menu(MenuAction::Revert)) => {
                if let Some(server) = server.as_ref() {
                    revert(server, &mut windows, target, &proxy);
                }
                None
            }
            Event::UserEvent(UserEvent::Menu(action)) => {
                if let Some(script) = action.page_script() {
                    windows.in_main(target, &proxy, script);
                }
                None
            }
            Event::UserEvent(UserEvent::Dropped(path)) => {
                if let Some(server) = server.as_ref() {
                    open_project(server, &mut windows, target, &proxy, &path);
                }
                None
            }
            // A .signal double-clicked in the Finder (or dropped on the Dock
            // icon): tao's open-file event.
            Event::Opened { urls } => {
                let path = urls.iter().find_map(|url| url.to_file_path().ok());
                if let (Some(path), Some(server)) = (path, server.as_ref()) {
                    open_project(server, &mut windows, target, &proxy, &path);
                }
                None
            }
            Event::UserEvent(UserEvent::Project(status)) => {
                menu.set_project(status.as_ref());
                let dir = status.as_ref().map(|s| s.dir.clone());
                let old = std::mem::replace(&mut windows.project, status);
                windows.titles();
                // Another project: its layout for these screens.
                if dir.is_some() && dir != windows.dir {
                    windows.flush_to(old.as_ref());
                    windows.dir = dir;
                    let layout = windows.stored();
                    windows.apply(target, &proxy, &layout);
                }
                None
            }
            Event::UserEvent(UserEvent::Page(id, text)) => {
                windows.page(target, &proxy, id, &text);
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
        // The layout's last change first, so the prompt asks about it too.
        windows.save(running, true);
        let Some(discard) = closing(running, &mut windows, target, &proxy, route) else {
            return;
        };
        let size = windows.main_size();
        if let Some(mut server) = server.take() {
            server.exit(size, discard);
        }
        *control_flow = ControlFlow::Exit;
    })
}

/// Before the app closes by `route`: an edited `.signal` project's prompt.
/// `None` stays (Cancel, or a failed save); else whether the edits are let
/// go (an unanswered prompt keeps them in the working folder).
fn closing(
    server: &AppServer,
    windows: &mut Windows,
    target: &Target,
    proxy: &Proxy,
    route: QuitRoute,
) -> Option<bool> {
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
        ExitPlan::Save => {
            match server.save() {
                Ok(()) => Some(false),
                // Terminating cannot stay: the working folder keeps the edits.
                Err(message) if route == QuitRoute::Terminate => {
                    eprintln!("signal-app: {message}");
                    Some(false)
                }
                Err(message) => {
                    windows.tell(target, proxy, "signal-app-error", &message);
                    None
                }
            }
        }
    }
}

/// Opens what was chosen, dropped or opened in the Finder: refused unless a
/// folder, `project.toml` or `.signal` file; an edited project is asked
/// about first. The layout's last change is written to the project it is
/// of first.
fn open_project(
    server: &AppServer,
    windows: &mut Windows,
    target: &Target,
    proxy: &Proxy,
    path: &Path,
) {
    let target_path = match dropped(path) {
        Ok(target_path) => target_path,
        Err(refusal) => return windows.tell(target, proxy, "signal-app-error", refusal),
    };
    windows.save(server, true);
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
    if let Err(message) = server.open(target_path, edits) {
        windows.tell(target, proxy, "signal-app-error", &message);
    }
}

/// Revert to Saved…: asked first ("Revert to the last saved version of
/// <name>?", Cancel the default); Revert opens the `.signal` file again
/// through the server, as an open of it with its changes let go (a save
/// still running is waited for, both by the look at the project here and
/// by the open). Nothing is asked when there is nothing to revert, and the
/// layout's pending change is not written: the file's `ui/` comes back.
fn revert(server: &AppServer, windows: &mut Windows, target: &Target, proxy: &Proxy) {
    let project = server.project();
    let answer = match &project {
        Some(shown) if can_revert(Some(shown)) => ask_revert(&shown.name),
        _ => None,
    };
    if revert_plan(project.as_ref(), answer) == RevertPlan::Revert
        && let Err(message) = server.revert()
    {
        windows.tell(target, proxy, "signal-app-error", &message);
    }
}

/// "Revert to the last saved version of <name>?" as a native alert
/// (NSAlert, modal, safe API, as the "Save changes?" prompt) with Cancel
/// first and Revert, each given its key equivalent explicitly (Escape on
/// Cancel, none on Revert: `RevertPrompt::key_equivalent`), Revert marked
/// destructive; the answer, or None when it cannot run.
fn ask_revert(name: &str) -> Option<RevertPrompt> {
    let mtm = MainThreadMarker::new()?;
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&revert_text(name)));
    alert.setInformativeText(&NSString::from_str(REVERT_INFORMATIVE));
    for button in REVERT_BUTTONS {
        let added = alert.addButtonWithTitle(&NSString::from_str(button.label()));
        added.setKeyEquivalent(&NSString::from_str(button.key_equivalent()));
        added.setHasDestructiveAction(button.destructive());
    }
    let index = alert.runModal().checked_sub(NSAlertFirstButtonReturn)?;
    RevertPrompt::of(usize::try_from(index).ok()?)
}

/// Save As…: the system's save panel with `<project name>.signal`; the
/// project is that file from then on.
fn save_as(server: &AppServer, windows: &mut Windows, target: &Target, proxy: &Proxy) {
    // The name only: never waits for a save still running.
    let name = default_save_name(server.project_shown().as_ref());
    let Some(file) = choose_save(&name) else {
        return;
    };
    let file = match save_target(file) {
        Ok(file) => file,
        Err(message) => return windows.tell(target, proxy, "signal-app-error", &message),
    };
    windows.save(server, true);
    match server.save_as(&file) {
        Ok(()) => windows.tell(
            target,
            proxy,
            "signal-app-notice",
            &format!("saved as {}", file.display()),
        ),
        Err(message) => windows.tell(target, proxy, "signal-app-error", &message),
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
