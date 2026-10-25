//! What the window decides without a window: its native menu bar (as data;
//! `muda` builds it on the main thread), what each of Signal's own items
//! does and its keymap command (ADR 0034: its chord is the command's), where the WebView may navigate, and the size recorded in the Setup;
//! and for `.signal` projects (M2.8-12): what a drop or a choice opens, the
//! window's title, Save As's default name, what the "Save changes?"
//! prompt offers and leads to on each way of quitting, and when Revert to
//! Saved… applies and what its prompt leads to. For several windows
//! over the screens (ADR 0032): what each view loads and how it is titled
//! (the layout itself is [`crate::layout`]).

use std::path::{Path, PathBuf};

use signal_project::archive;
use signal_ui::keys::Keys;
use signal_ui::{APP_PAGE, ProjectStatus, SCHEMATIC_PAGE};

use crate::layout::WindowView;
use crate::setup::Window;

/// What one of Signal's own menu items does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Open…: the system's chooser (NSOpenPanel, in `main.rs`); a chosen
    /// folder, `project.toml` or `.signal` file opens as a dropped one does.
    /// Without the main thread, the page shows the Setup tab with its folder
    /// field focused (ADR 0023).
    Open,
    /// Save: the page writes every unsaved parameter again, then a
    /// `.signal` project's working folder back to its file.
    Save,
    /// Save As…: the system's save panel (NSSavePanel, in `main.rs`); the
    /// project is written to the chosen `.signal` file and is that file
    /// from then on.
    SaveAs,
    /// Revert to Saved…: after the window's confirmation ([`revert_plan`]),
    /// the open `.signal` project opened again from its file, its changes
    /// let go; enabled only while [`can_revert`].
    Revert,
    /// Interface Setup…: the page shows the Setup tab.
    Setup,
    /// Quit: as closing the last window (the Setup records the open
    /// windows).
    Quit,
    /// Window ▸ New Main Window (ADR 0032): another window on the app's page.
    NewMain,
    /// Window ▸ New Schematic Window: a window on the schematic alone.
    NewSchematic,
    /// Window ▸ New Panel Window: an empty panel window, to drop cards in.
    NewPanel,
    /// Window ▸ Reset Layout: everything back on one main window on the
    /// main screen; the screen configuration's layout is removed from
    /// `ui/windows.toml`.
    ResetLayout,
    /// Project ▸ Restart from Files (ADR 0033): the page sends
    /// `transport.play`, which plays the open project again from frame 0
    /// from its files. What the toolbar's Play did: the toolbar has no Play
    /// or Stop, the project runs while the power is on, and its own
    /// transport starts and stops it.
    Restart,
    /// Signal ▸ Settings… (ADR 0034): the Settings window, on its Keyboard
    /// Shortcuts page.
    Settings,
    /// Help ▸ Keyboard Shortcuts: the Settings window's shortcuts as a
    /// reference (read first, edited from there).
    KeysHelp,
}

impl MenuAction {
    const ALL: [MenuAction; 13] = [
        MenuAction::Open,
        MenuAction::Save,
        MenuAction::SaveAs,
        MenuAction::Revert,
        MenuAction::Setup,
        MenuAction::Quit,
        MenuAction::NewMain,
        MenuAction::NewSchematic,
        MenuAction::NewPanel,
        MenuAction::ResetLayout,
        MenuAction::Restart,
        MenuAction::Settings,
        MenuAction::KeysHelp,
    ];

    /// The menu item's id.
    pub fn id(self) -> &'static str {
        match self {
            MenuAction::Open => "open",
            MenuAction::Save => "save",
            MenuAction::SaveAs => "save-as",
            MenuAction::Revert => "revert",
            MenuAction::Setup => "setup",
            MenuAction::Quit => "quit",
            MenuAction::NewMain => "new-main-window",
            MenuAction::NewSchematic => "new-schematic-window",
            MenuAction::NewPanel => "new-panel-window",
            MenuAction::ResetLayout => "reset-layout",
            MenuAction::Restart => "restart",
            MenuAction::Settings => "settings",
            MenuAction::KeysHelp => "keyboard-shortcuts",
        }
    }

    /// Its command in the keymap (`ui/lib/keys.default.js`, ADR 0034),
    /// whose first key chord is the item's.
    pub fn command(self) -> &'static str {
        match self {
            MenuAction::Open => "file.open",
            MenuAction::Save => "file.save",
            MenuAction::SaveAs => "file.save_as",
            MenuAction::Revert => "file.revert",
            MenuAction::Setup => "file.interface_setup",
            MenuAction::Quit => "app.quit",
            MenuAction::NewMain => "window.new_main",
            MenuAction::NewSchematic => "window.new_schematic",
            MenuAction::NewPanel => "window.new_panel",
            MenuAction::ResetLayout => "window.reset_layout",
            MenuAction::Restart => "transport.restart",
            MenuAction::Settings => "app.settings",
            MenuAction::KeysHelp => "help.keyboard_shortcuts",
        }
    }

    /// The action of keymap command `id` (a page's `menu.run`), if it is a
    /// menu item's.
    pub fn of_command(id: &str) -> Option<MenuAction> {
        MenuAction::ALL.into_iter().find(|a| a.command() == id)
    }

    /// The action of the menu item `id`, if it is one of Signal's own.
    pub fn of(id: &str) -> Option<MenuAction> {
        MenuAction::ALL.into_iter().find(|a| a.id() == id)
    }

    /// The script that hands the action to the page (a `signal-menu`
    /// event, `ui/app/app.js`, so a main window's); none for Save As,
    /// Revert to Saved, Quit, the new windows and Settings, which the app
    /// does.
    pub fn page_script(self) -> Option<String> {
        (!matches!(
            self,
            MenuAction::Quit
                | MenuAction::SaveAs
                | MenuAction::Revert
                | MenuAction::NewMain
                | MenuAction::NewSchematic
                | MenuAction::NewPanel
                | MenuAction::ResetLayout
                | MenuAction::Settings
                | MenuAction::KeysHelp
        ))
        .then(|| {
            let quoted = serde_json::to_string(self.id()).unwrap_or_else(|_| "\"\"".to_owned());
            format!("window.dispatchEvent(new CustomEvent('signal-menu', {{ detail: {quoted} }}))")
        })
    }
}

/// An item of the menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    /// One of Signal's own: its action and label; its chord is its
    /// command's first in the keymap (ADR 0034), none without one.
    Own(MenuAction, &'static str),

    // The system's items (`muda`'s predefined ones) act on the app or, for
    // the Edit menu, on the focused text field.
    /// About Signal.
    About,
    /// Hide Signal.
    Hide,
    /// Hide the other apps.
    HideOthers,
    /// Show every app.
    ShowAll,
    /// Undo (in a text field).
    Undo,
    /// Redo.
    Redo,
    /// Cut.
    Cut,
    /// Copy.
    Copy,
    /// Paste.
    Paste,
    /// Select All.
    SelectAll,
    /// Minimize the focused window.
    Minimize,
    /// Zoom it.
    Zoom,
    /// Enter or leave full screen (on the screen the window is on).
    Fullscreen,
    /// Bring every window of Signal to the front.
    BringAllToFront,
    /// A line between items.
    Separator,
}

/// The Window menu's name; the system adds the list of open windows to it.
pub const WINDOW_MENU: &str = "Window";

/// The Help menu's name.
pub const HELP_MENU: &str = "Help";

const QUIT: MenuItem = MenuItem::Own(MenuAction::Quit, "Quit Signal");

/// The menu bar: the app menu, File, Edit, Project, Window and Help, in
/// order (ADR 0023, 0032, 0033, 0034).
pub const MENU_BAR: [(&str, &[MenuItem]); 6] = [
    (
        "Signal",
        &[
            MenuItem::About,
            MenuItem::Separator,
            MenuItem::Own(MenuAction::Settings, "Settings…"),
            MenuItem::Separator,
            MenuItem::Hide,
            MenuItem::HideOthers,
            MenuItem::ShowAll,
            MenuItem::Separator,
            QUIT,
        ],
    ),
    (
        "File",
        &[
            MenuItem::Own(MenuAction::Open, "Open…"),
            MenuItem::Own(MenuAction::Save, "Save"),
            MenuItem::Own(MenuAction::SaveAs, "Save As…"),
            MenuItem::Own(MenuAction::Revert, "Revert to Saved…"),
            MenuItem::Separator,
            MenuItem::Own(MenuAction::Setup, "Interface Setup…"),
            MenuItem::Separator,
            MenuItem::Own(MenuAction::Quit, "Quit"),
        ],
    ),
    (
        "Edit",
        &[
            MenuItem::Undo,
            MenuItem::Redo,
            MenuItem::Separator,
            MenuItem::Cut,
            MenuItem::Copy,
            MenuItem::Paste,
            MenuItem::SelectAll,
        ],
    ),
    (
        "Project",
        &[MenuItem::Own(MenuAction::Restart, "Restart from Files")],
    ),
    (
        WINDOW_MENU,
        &[
            MenuItem::Own(MenuAction::NewMain, "New Main Window"),
            MenuItem::Own(MenuAction::NewSchematic, "New Schematic Window"),
            MenuItem::Own(MenuAction::NewPanel, "New Panel Window"),
            MenuItem::Separator,
            MenuItem::Own(MenuAction::ResetLayout, "Reset Layout"),
            MenuItem::Separator,
            MenuItem::Minimize,
            MenuItem::Zoom,
            MenuItem::Fullscreen,
            MenuItem::Separator,
            MenuItem::BringAllToFront,
        ],
    ),
    (
        HELP_MENU,
        &[MenuItem::Own(MenuAction::KeysHelp, "Keyboard Shortcuts")],
    ),
];

/// Each of Signal's own menu actions and its chord in `keys` (the
/// command's first key chord, as `muda` parses it), none without one.
pub fn menu_chords(keys: &Keys) -> Vec<(MenuAction, Option<String>)> {
    let chords = keys.menu_chords();
    MenuAction::ALL
        .into_iter()
        .map(|action| {
            let chord = chords
                .iter()
                .find(|(id, _)| id == action.command())
                .and_then(|(_, chord)| chord.clone());
            (action, chord)
        })
        .collect()
}

/// The Settings page (ADR 0034): its own window, not part of a project's
/// layout; `reference` opens its shortcuts as a reference (Help ▸ Keyboard
/// Shortcuts).
pub fn settings_path(reference: bool) -> &'static str {
    if reference {
        "/ui/app/settings.html?view=reference"
    } else {
        "/ui/app/settings.html"
    }
}

/// The Settings window's title.
pub const SETTINGS_TITLE: &str = "Signal — Settings";

/// The path (from `/`) a window of `view` loads: a panel window's is the
/// card host page (`ui/app/panel-window.html`), which shows the project's main
/// panel in a frame, only the cards that window holds.
pub fn view_path(view: WindowView) -> &'static str {
    match view {
        WindowView::Main => APP_PAGE,
        WindowView::Schematic => SCHEMATIC_PAGE,
        WindowView::Panel => PANEL_WINDOW_PAGE,
    }
}

/// A panel window's page in the UI library.
pub const PANEL_WINDOW_PAGE: &str = "/ui/app/panel-window.html";

/// The URL of `path` on the app's server: `app_url`'s origin and fragment
/// (the WebSocket's port and the session token), so every window is a
/// client of the same server.
pub fn page_url(app_url: &str, path: &str) -> String {
    let fragment = app_url
        .find('#')
        .and_then(|i| app_url.get(i..))
        .unwrap_or("");
    format!("{}{path}{fragment}", origin_of(app_url))
}

/// The title of a window of `view`: a main window's is [`title`]; the
/// others say what they show, then the project's title.
pub fn view_title(view: WindowView, project: Option<&ProjectStatus>) -> String {
    window_title(view, "", project)
}

/// The title of a window of `view` named `name` (`layout::labels`: "Panel
/// window 2 (screen 3)", the screen left out of the title): a main
/// window's is [`title`]; the others say which window they are, then the
/// project's title.
pub fn window_title(view: WindowView, name: &str, project: Option<&ProjectStatus>) -> String {
    let name = name.split(" (screen").next().unwrap_or(name);
    if !name.is_empty() && view != WindowView::Main {
        return match project {
            Some(_) => format!("Signal — {name} — {}", title(project)),
            None => format!("Signal — {name}"),
        };
    }
    view_title_plain(view, project)
}

fn view_title_plain(view: WindowView, project: Option<&ProjectStatus>) -> String {
    let shows = match view {
        WindowView::Main => return title(project),
        WindowView::Schematic => "Schematic",
        WindowView::Panel => "Panel",
    };
    match project {
        Some(_) => format!("Signal — {shows} — {}", title(project)),
        None => format!("Signal — {shows}"),
    }
}

/// What the WebView runs before the page: it tells the page that the
/// native menu takes the shortcuts (`ui/app/lib/menu.js`).
pub const NATIVE_MENU_SCRIPT: &str = "window.signalApp = Object.freeze({ menu: true });";

/// Whether the WebView may navigate to `url`: only the app's own server
/// (`origin`, `http://127.0.0.1:<port>`), never another site.
pub fn may_navigate(url: &str, origin: &str) -> bool {
    url.strip_prefix(origin)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The origin of the page URL `url` (`http://127.0.0.1:<port>`).
pub fn origin_of(url: &str) -> &str {
    let after = url.find("://").map_or(0, |i| i + 3);
    match url.get(after..).and_then(|rest| rest.find('/')) {
        Some(slash) => url.get(..after + slash).unwrap_or(url),
        None => url,
    }
}

/// What dropping (or choosing) `path` does: a folder is opened as the
/// project, a project's `project.toml` opens its folder, and a `.signal`
/// file opens as one; anything else is refused with the message the window
/// shows, and opens nothing (so it never becomes the Setup's last project).
pub fn dropped(path: &Path) -> Result<&Path, &'static str> {
    if path.is_dir() || (archive::is_project_file(path) && path.is_file()) {
        Ok(path)
    } else if path.file_name().is_some_and(|name| name == "project.toml") && path.is_file() {
        path.parent().ok_or("open a project folder")
    } else {
        Err("open a project folder, its project.toml or a .signal file")
    }
}

/// The window's title: the project's file (or folder) name, with
/// " — Edited" while a `.signal` project's working folder differs from it.
pub fn title(project: Option<&ProjectStatus>) -> String {
    let Some(project) = project else {
        return "Signal".to_owned();
    };
    let shown = project
        .file
        .as_deref()
        .and_then(Path::file_name)
        .map_or_else(
            || project.name.clone(),
            |n| n.to_string_lossy().into_owned(),
        );
    if project.edited {
        format!("{shown} — Edited")
    } else {
        shown
    }
}

/// The file Save As writes for the `chosen` path: it, with `.signal`
/// added when the name was typed without it. The panel asked about
/// replacing only `chosen`, so a different file that already exists is
/// refused rather than replaced unasked.
pub fn save_target(chosen: PathBuf) -> Result<PathBuf, String> {
    let file = with_signal_extension(chosen.clone());
    if file != chosen && file.exists() {
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        return Err(format!(
            "{name} already exists: choose it in Save As to replace it"
        ));
    }
    Ok(file)
}

/// Save As…'s default name: `<project name>.signal`.
pub fn default_save_name(project: Option<&ProjectStatus>) -> String {
    let name = project.map_or("Untitled", |p| p.name.as_str());
    format!("{name}.{}", archive::EXTENSION)
}

/// The file Save As writes: the chosen path, `.signal` added when the
/// name was typed without it.
pub fn with_signal_extension(path: PathBuf) -> PathBuf {
    if archive::is_project_file(&path) {
        path
    } else {
        let mut name = path.file_name().unwrap_or_default().to_owned();
        name.push(".");
        name.push(archive::EXTENSION);
        path.with_file_name(name)
    }
}

/// How the app is being quit, or its project replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitRoute {
    /// The window's close button.
    Window,
    /// File ▸ Quit, Signal ▸ Quit Signal, ⌘Q (Signal's own menu item).
    Menu,
    /// Another project opened (the chooser, a drop, the Finder).
    Open,
    /// The Dock's Quit, or logout: macOS has already decided to terminate.
    /// The locked tao 0.37.1 never answers `applicationShouldTerminate:`,
    /// and answering it needs an unsafe `NSApplicationDelegate`, so this
    /// prompt cannot offer Cancel (operator decision operator-request:009,
    /// ADR 0025).
    Terminate,
}

/// A button of the "Save changes?" prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// Save the project to its file, then go on.
    Save,
    /// Stay: nothing is closed or opened.
    Cancel,
    /// Let the edits go, then go on.
    DontSave,
}

impl Prompt {
    /// The button's title.
    pub fn label(self) -> &'static str {
        match self {
            Prompt::Save => "Save",
            Prompt::Cancel => "Cancel",
            Prompt::DontSave => "Don't Save",
        }
    }

    /// The button answered: the `index`th of [`prompt_buttons`] (the
    /// alert's first, second or third button); none for another answer.
    pub fn of(route: QuitRoute, index: usize) -> Option<Prompt> {
        prompt_buttons(route).get(index).copied()
    }
}

/// The prompt's text: "Save changes to <name>?".
pub fn prompt_text(name: &str) -> String {
    format!("Save changes to {name}?")
}

/// The prompt's buttons, the default first: Save, Cancel and Don't Save,
/// except when terminating, which cannot be cancelled.
pub fn prompt_buttons(route: QuitRoute) -> &'static [Prompt] {
    match route {
        QuitRoute::Terminate => &[Prompt::Save, Prompt::DontSave],
        _ => &[Prompt::Save, Prompt::Cancel, Prompt::DontSave],
    }
}

/// What happens next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitPlan {
    /// Not edited: go on, nothing to ask.
    Go,
    /// Cancel: stay.
    Stay,
    /// Save, then go on (stay if the save fails).
    Save,
    /// Go on, letting the edits go.
    Discard,
    /// No answer (the alert could not run): go on, and keep the working
    /// folder with the edits.
    Keep,
}

/// What an edited project's prompt answered leads to; `answer` is `None`
/// when the prompt was not answered.
pub fn exit_plan(edited: bool, answer: Option<Prompt>) -> ExitPlan {
    match (edited, answer) {
        (false, _) => ExitPlan::Go,
        (true, Some(Prompt::Save)) => ExitPlan::Save,
        (true, Some(Prompt::Cancel)) => ExitPlan::Stay,
        (true, Some(Prompt::DontSave)) => ExitPlan::Discard,
        (true, None) => ExitPlan::Keep,
    }
}

/// Whether File ▸ Revert to Saved… is enabled for `project`: a `.signal`
/// project with changes since it was last saved or opened (edited, or
/// unsaved live values). Never for a folder project: Signal writes it
/// straight to its files, so there is no saved version to go back to.
pub fn can_revert(project: Option<&ProjectStatus>) -> bool {
    project.is_some_and(ProjectStatus::revertible)
}

/// A button of the "Revert to Saved?" prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevertPrompt {
    /// Stay: nothing is let go.
    Cancel,
    /// Let the changes go and open the file again.
    Revert,
}

impl RevertPrompt {
    /// The button's title.
    pub fn label(self) -> &'static str {
        match self {
            RevertPrompt::Cancel => "Cancel",
            RevertPrompt::Revert => "Revert",
        }
    }

    /// The key equivalent the alert's button is given explicitly, never
    /// left to NSAlert's defaults: Escape for Cancel, none for Revert.
    ///
    /// NSAlert gives its first button Return and a button titled "Cancel"
    /// Escape (Apple's `addButtonWithTitle:` documentation), but an
    /// `NSButton` holds one key equivalent, so a Cancel that is the first
    /// button cannot hold both, and the documentation does not say which
    /// wins. A second button that also cancels would need a target and an
    /// action, which objc2-app-kit sets only through `unsafe` calls (denied
    /// workspace-wide). So Escape is made certain on Cancel, and Revert has
    /// no key at all: Return reverts nothing (no button takes it), and only
    /// a click on Revert reverts.
    pub fn key_equivalent(self) -> &'static str {
        match self {
            RevertPrompt::Cancel => "\u{1b}",
            RevertPrompt::Revert => "",
        }
    }

    /// The button answered: the `index`th of [`REVERT_BUTTONS`]; none for
    /// another answer.
    pub fn of(index: usize) -> Option<RevertPrompt> {
        REVERT_BUTTONS.get(index).copied()
    }
}

/// The revert prompt's buttons, the most prominent first: Cancel (with
/// Escape, [`RevertPrompt::key_equivalent`]), then Revert.
pub const REVERT_BUTTONS: [RevertPrompt; 2] = [RevertPrompt::Cancel, RevertPrompt::Revert];

/// The revert prompt's text: "Revert to the last saved version of <name>?".
pub fn revert_text(name: &str) -> String {
    format!("Revert to the last saved version of {name}?")
}

/// The revert prompt's informative text.
pub const REVERT_INFORMATIVE: &str = "Your changes since then are lost.";

/// What Revert to Saved… leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevertPlan {
    /// Nothing to revert (no `.signal` project, or no change): nothing is
    /// asked or done.
    Nothing,
    /// Cancel, or no answer (the alert could not run): stay, nothing is
    /// let go.
    Stay,
    /// Let the changes go and open the file again.
    Revert,
}

/// What Revert to Saved… for `project` leads to, `answer` being the
/// prompt's (`None` when it was not answered); only Revert lets anything go.
pub fn revert_plan(project: Option<&ProjectStatus>, answer: Option<RevertPrompt>) -> RevertPlan {
    match (can_revert(project), answer) {
        (false, _) => RevertPlan::Nothing,
        (true, Some(RevertPrompt::Revert)) => RevertPlan::Revert,
        (true, Some(RevertPrompt::Cancel) | None) => RevertPlan::Stay,
    }
}

/// The window's size in points, as the Setup records it (the default main
/// window's size, ADR 0032).
pub fn window_of(width: f64, height: f64) -> Option<Window> {
    (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0).then_some(Window {
        width: width as f32,
        height: height as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The items of `menu` and their chords in the built-in keymap.
    fn own(menu: &str) -> Vec<(&'static str, Option<String>)> {
        let chords = menu_chords(&Keys::built_in(Path::new("keys.toml")));
        MENU_BAR
            .iter()
            .find(|(name, _)| *name == menu)
            .unwrap()
            .1
            .iter()
            .filter_map(|item| match item {
                MenuItem::Own(action, label) => {
                    let chord = chords.iter().find(|(a, _)| a == action).unwrap().1.clone();
                    Some((*label, chord))
                }
                _ => None,
            })
            .collect()
    }

    fn chord(text: &str) -> Option<String> {
        Some(text.to_owned())
    }

    /// M2.8-11 AC12, AC15: the app menu, File and Edit (and Window, ADR
    /// 0032; Project, ADR 0033, with Restart from Files, handed to the
    /// page, which sends `transport.play`; Settings… and Help, ADR 0034).
    /// The chords are the keymap's (`ui/lib/keys.default.js`).
    #[test]
    fn the_menu_bar_has_the_app_menu_file_and_edit() {
        let names: Vec<&str> = MENU_BAR.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            ["Signal", "File", "Edit", "Project", "Window", "Help"]
        );
        assert_eq!(
            own("File"),
            [
                ("Open…", chord("Cmd+O")),
                ("Save", chord("Cmd+S")),
                ("Save As…", chord("Shift+Cmd+S")),
                ("Revert to Saved…", None),
                ("Interface Setup…", chord("Alt+Cmd+,")),
                ("Quit", chord("Cmd+Q"))
            ]
        );
        assert_eq!(
            own("Signal"),
            [
                ("Settings…", chord("Cmd+,")),
                ("Quit Signal", chord("Cmd+Q"))
            ]
        );
        assert_eq!(own("Project"), [("Restart from Files", None)]);
        assert_eq!(own("Help"), [("Keyboard Shortcuts", chord("Cmd+/"))]);
        assert_eq!(
            MenuAction::Restart.page_script().unwrap(),
            "window.dispatchEvent(new CustomEvent('signal-menu', { detail: \"restart\" }))"
        );
        let (_, app) = MENU_BAR[0];
        for item in [MenuItem::About, MenuItem::Hide, QUIT] {
            assert!(app.contains(&item), "{item:?}");
        }
        let (_, edit) = MENU_BAR[2];
        let edit: Vec<MenuItem> = edit
            .iter()
            .copied()
            .filter(|i| *i != MenuItem::Separator)
            .collect();
        assert_eq!(
            edit,
            [
                MenuItem::Undo,
                MenuItem::Redo,
                MenuItem::Cut,
                MenuItem::Copy,
                MenuItem::Paste,
                MenuItem::SelectAll
            ]
        );
    }

    /// ADR 0032: the Window menu opens main, schematic and panel windows,
    /// resets the layout, and has the system's window items; no chord is
    /// taken twice (ADR 0034: the keymap's conflicts).
    #[test]
    fn the_window_menu_opens_more_windows() {
        let (_, window) = MENU_BAR[4];
        assert_eq!(
            window,
            [
                MenuItem::Own(MenuAction::NewMain, "New Main Window"),
                MenuItem::Own(MenuAction::NewSchematic, "New Schematic Window"),
                MenuItem::Own(MenuAction::NewPanel, "New Panel Window"),
                MenuItem::Separator,
                MenuItem::Own(MenuAction::ResetLayout, "Reset Layout"),
                MenuItem::Separator,
                MenuItem::Minimize,
                MenuItem::Zoom,
                MenuItem::Fullscreen,
                MenuItem::Separator,
                MenuItem::BringAllToFront,
            ]
        );
        assert_eq!(own("Window")[0], ("New Main Window", chord("Shift+Cmd+N")));
        let chords = menu_chords(&Keys::built_in(Path::new("keys.toml")));
        let mut taken: Vec<String> = chords.iter().filter_map(|(_, c)| c.clone()).collect();
        let all = taken.len();
        taken.sort_unstable();
        taken.dedup();
        assert_eq!(taken.len(), all, "a chord taken twice: {taken:?}");
        // Minimize ⌘M, Hide ⌘H, full screen ⌃⌘F, the Edit menu's: none of
        // Signal's own.
        for system in [
            "Cmd+M",
            "Cmd+H",
            "Cmd+Z",
            "Cmd+X",
            "Cmd+C",
            "Cmd+V",
            "Cmd+A",
            "Ctrl+Cmd+F",
        ] {
            assert!(!taken.iter().any(|c| c == system), "{system}");
        }
        for action in [
            MenuAction::NewMain,
            MenuAction::NewSchematic,
            MenuAction::NewPanel,
            MenuAction::ResetLayout,
            MenuAction::Settings,
            MenuAction::KeysHelp,
        ] {
            assert_eq!(action.page_script(), None, "{action:?}: the app does it");
            assert_eq!(MenuAction::of(action.id()), Some(action));
        }
    }

    /// ADR 0034: every menu action is a menu command of the keymap, and
    /// every menu command is one of the menu's actions; a chord from
    /// `keys.toml` is the item's.
    #[test]
    fn the_menu_s_chords_are_the_keymap_s() {
        let defaults = signal_ui::keys::defaults();
        let menu: Vec<&str> = defaults
            .commands
            .iter()
            .filter(|c| c.menu)
            .map(|c| c.id.as_str())
            .collect();
        for action in MenuAction::ALL {
            assert!(menu.contains(&action.command()), "{action:?}");
            assert_eq!(MenuAction::of_command(action.command()), Some(action));
        }
        assert_eq!(menu.len(), MenuAction::ALL.len());
        assert_eq!(MenuAction::of_command("view.page_down"), None);
        let keys = Keys::parse(
            "[window]\nnew_schematic = [\"Shift+Cmd+N\", \"F5\"]\nnew_main = []\n",
            Path::new("keys.toml"),
        );
        let chords = menu_chords(&keys);
        let of = |action| chords.iter().find(|(a, _)| *a == action).unwrap().1.clone();
        assert_eq!(of(MenuAction::NewSchematic), chord("Shift+Cmd+N"));
        assert_eq!(of(MenuAction::NewMain), None);
        assert_eq!(settings_path(true), "/ui/app/settings.html?view=reference");
    }

    /// ADR 0032: every view is a page of the same server, with the same
    /// fragment, so each window is one more client; titles say the view.
    #[test]
    fn each_view_is_a_page_of_the_app_s_server() {
        let app = "http://127.0.0.1:5123/ui/app/index.html#ws=5124&token=ab";
        let url = |view| page_url(app, view_path(view));
        assert_eq!(url(WindowView::Main), app);
        assert_eq!(
            url(WindowView::Schematic),
            "http://127.0.0.1:5123/schematic#ws=5124&token=ab"
        );
        assert_eq!(
            url(WindowView::Panel),
            "http://127.0.0.1:5123/ui/app/panel-window.html#ws=5124&token=ab"
        );
        for view in [WindowView::Main, WindowView::Schematic, WindowView::Panel] {
            assert!(may_navigate(&url(view), origin_of(app)));
        }
        let kick = status(Some("/p/kick.signal"), "kick", true);
        assert_eq!(
            view_title(WindowView::Main, Some(&kick)),
            "kick.signal — Edited"
        );
        assert_eq!(
            view_title(WindowView::Schematic, Some(&kick)),
            "Signal — Schematic — kick.signal — Edited"
        );
        assert_eq!(
            view_title(WindowView::Panel, Some(&kick)),
            "Signal — Panel — kick.signal — Edited"
        );
        assert_eq!(view_title(WindowView::Panel, None), "Signal — Panel");
        assert_eq!(view_title(WindowView::Main, None), "Signal");
        assert_eq!(
            window_title(WindowView::Panel, "Panel window 2 (screen 3)", Some(&kick)),
            "Signal — Panel window 2 — kick.signal — Edited"
        );
        assert_eq!(
            window_title(WindowView::Main, "Main window 2", None),
            "Signal",
            "a main window's is the project's"
        );
    }

    #[test]
    fn each_own_item_has_its_action_and_the_page_gets_all_but_quit() {
        for (_, items) in MENU_BAR {
            for item in items {
                if let MenuItem::Own(action, _) = item {
                    assert_eq!(MenuAction::of(action.id()), Some(*action));
                }
            }
        }
        assert_eq!(MenuAction::of("rm -rf"), None);
        assert_eq!(
            MenuAction::Open.page_script().unwrap(),
            "window.dispatchEvent(new CustomEvent('signal-menu', { detail: \"open\" }))"
        );
        assert!(MenuAction::Save.page_script().unwrap().contains("\"save\""));
        assert!(
            MenuAction::Setup
                .page_script()
                .unwrap()
                .contains("\"setup\"")
        );
        assert_eq!(MenuAction::Quit.page_script(), None, "the window quits");
        assert_eq!(
            MenuAction::SaveAs.page_script(),
            None,
            "the window's save panel"
        );
        assert_eq!(
            MenuAction::Revert.page_script(),
            None,
            "the window's prompt"
        );
    }

    #[test]
    fn only_the_app_s_server_is_navigated_to() {
        let url = "http://127.0.0.1:5123/ui/app/index.html#ws=5124&token=ab";
        let origin = origin_of(url);
        assert_eq!(origin, "http://127.0.0.1:5123");
        assert!(may_navigate(url, origin));
        assert!(may_navigate(
            "http://127.0.0.1:5123/project/main.html#ws=1",
            origin
        ));
        assert!(!may_navigate("http://127.0.0.1:51234/", origin));
        assert!(!may_navigate("http://127.0.0.1:5123.evil.example/", origin));
        assert!(!may_navigate("https://example.com/", origin));
    }

    #[test]
    fn only_a_dropped_folder_is_opened() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert_eq!(dropped(dir), Ok(dir));
        assert_eq!(
            dropped(&dir.join("Cargo.toml")),
            Err("open a project folder, its project.toml or a .signal file")
        );
        let kick = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../projects/kick"));
        assert_eq!(dropped(&kick.join("project.toml")), Ok(kick), "its folder");
        assert!(dropped(&dir.join("project.toml")).is_err(), "no such file");
    }

    /// M2.8-12 AC13: a .signal file is opened as one; a missing one is not.
    #[test]
    fn a_dropped_signal_file_is_opened() {
        let base = std::env::temp_dir().join(format!("signal-window-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let file = base.join("kick.signal");
        std::fs::write(&file, "zip").unwrap();
        assert_eq!(dropped(&file), Ok(file.as_path()));
        assert!(dropped(&base.join("gone.signal")).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }

    fn status(file: Option<&str>, name: &str, edited: bool) -> ProjectStatus {
        ProjectStatus {
            dir: PathBuf::from("/open/kick-1-1"),
            file: file.map(PathBuf::from),
            name: name.to_owned(),
            edited,
            unsaved: 0,
        }
    }

    /// M2.8-12 AC17, AC19: the title says "— Edited"; Save As offers
    /// <project name>.signal and always writes a .signal file.
    #[test]
    fn the_title_and_save_as_name_follow_the_project() {
        assert_eq!(title(None), "Signal");
        let file = status(Some("/p/kick.signal"), "kick", false);
        assert_eq!(title(Some(&file)), "kick.signal");
        let edited = status(Some("/p/kick.signal"), "kick", true);
        assert_eq!(title(Some(&edited)), "kick.signal — Edited");
        let folder = status(None, "pulse", false);
        assert_eq!(title(Some(&folder)), "pulse");
        assert_eq!(default_save_name(Some(&folder)), "pulse.signal");
        assert_eq!(default_save_name(Some(&file)), "kick.signal");
        assert_eq!(default_save_name(None), "Untitled.signal");
        assert_eq!(
            with_signal_extension(PathBuf::from("/p/kick")),
            PathBuf::from("/p/kick.signal")
        );
        assert_eq!(
            with_signal_extension(PathBuf::from("/p/kick.signal")),
            PathBuf::from("/p/kick.signal")
        );
        assert_eq!(
            with_signal_extension(PathBuf::from("/p/kick.v2")),
            PathBuf::from("/p/kick.v2.signal")
        );
    }

    /// Second-opinion review of M2.8-12: typing `kick` must not replace an
    /// existing `kick.signal` the panel never asked about.
    #[test]
    fn save_as_never_replaces_a_file_the_panel_did_not_ask_about() {
        let dir = std::env::temp_dir().join(format!("signal-save-target-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("kick.signal");
        std::fs::write(&existing, b"x").unwrap();
        assert!(
            save_target(dir.join("kick"))
                .unwrap_err()
                .contains("kick.signal already exists")
        );
        assert_eq!(
            save_target(existing.clone()),
            Ok(existing),
            "chosen itself: the panel asked"
        );
        assert_eq!(save_target(dir.join("pulse")), Ok(dir.join("pulse.signal")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// M2.8-12 AC20 and operator decision operator-request:009: the close
    /// button, the menu's Quit and opening another project offer Save, Don't
    /// Save and Cancel; the Dock's Quit and logout offer Save and Don't Save,
    /// and an unanswered prompt keeps the edits.
    #[test]
    fn the_prompt_offers_cancel_wherever_it_can() {
        assert_eq!(prompt_text("kick"), "Save changes to kick?");
        for route in [QuitRoute::Window, QuitRoute::Menu, QuitRoute::Open] {
            assert_eq!(
                prompt_buttons(route),
                [Prompt::Save, Prompt::Cancel, Prompt::DontSave],
                "{route:?}"
            );
            assert_eq!(Prompt::of(route, 1), Some(Prompt::Cancel));
            assert_eq!(Prompt::of(route, 2), Some(Prompt::DontSave));
        }
        assert_eq!(
            prompt_buttons(QuitRoute::Terminate),
            [Prompt::Save, Prompt::DontSave]
        );
        assert_eq!(Prompt::of(QuitRoute::Terminate, 1), Some(Prompt::DontSave));
        assert_eq!(Prompt::of(QuitRoute::Terminate, 2), None);
        let labels: Vec<&str> = prompt_buttons(QuitRoute::Menu)
            .iter()
            .map(|p| p.label())
            .collect();
        assert_eq!(labels, ["Save", "Cancel", "Don't Save"]);

        assert_eq!(exit_plan(false, None), ExitPlan::Go);
        assert_eq!(exit_plan(false, Some(Prompt::Cancel)), ExitPlan::Go);
        assert_eq!(exit_plan(true, Some(Prompt::Save)), ExitPlan::Save);
        assert_eq!(exit_plan(true, Some(Prompt::Cancel)), ExitPlan::Stay);
        assert_eq!(exit_plan(true, Some(Prompt::DontSave)), ExitPlan::Discard);
        assert_eq!(exit_plan(true, None), ExitPlan::Keep);
        let terminate = |i| exit_plan(true, Prompt::of(QuitRoute::Terminate, i));
        assert_eq!(terminate(0), ExitPlan::Save);
        assert_eq!(terminate(1), ExitPlan::Discard);
        assert_eq!(terminate(2), ExitPlan::Keep, "no Cancel when terminating");
    }

    /// The owner's request of 2026-10-25 ("jó lenne egy file revert is"):
    /// Revert to Saved… is enabled for a `.signal` project with changes
    /// (edited, or unsaved live values), never for a folder project; it
    /// asks with Cancel (the default) and Revert, and only Revert lets the
    /// changes go.
    #[test]
    fn revert_asks_first_and_only_for_a_changed_signal_file() {
        let saved = status(Some("/p/kick.signal"), "kick", false);
        let edited = status(Some("/p/kick.signal"), "kick", true);
        let live = ProjectStatus {
            unsaved: 2,
            ..saved.clone()
        };
        let folder = ProjectStatus {
            unsaved: 3,
            ..status(None, "pulse", true)
        };
        assert!(!can_revert(None));
        assert!(!can_revert(Some(&saved)), "nothing to go back from");
        assert!(can_revert(Some(&edited)));
        assert!(can_revert(Some(&live)), "unsaved live values");
        assert!(!can_revert(Some(&folder)), "a folder has no saved version");

        assert_eq!(
            revert_text("kick"),
            "Revert to the last saved version of kick?"
        );
        assert_eq!(REVERT_INFORMATIVE, "Your changes since then are lost.");
        let labels: Vec<&str> = REVERT_BUTTONS.iter().map(|b| b.label()).collect();
        assert_eq!(
            labels,
            ["Cancel", "Revert"],
            "Cancel first, the most prominent"
        );
        assert_eq!(RevertPrompt::of(0), Some(RevertPrompt::Cancel));
        assert_eq!(RevertPrompt::of(1), Some(RevertPrompt::Revert));
        assert_eq!(RevertPrompt::of(2), None);
        assert_eq!(RevertPrompt::Cancel.key_equivalent(), "\u{1b}", "Escape");
        assert_eq!(
            RevertPrompt::Revert.key_equivalent(),
            "",
            "no key reverts: never Return"
        );

        let edited = Some(&edited);
        assert_eq!(revert_plan(edited, RevertPrompt::of(1)), RevertPlan::Revert);
        assert_eq!(revert_plan(edited, RevertPrompt::of(0)), RevertPlan::Stay);
        assert_eq!(revert_plan(edited, None), RevertPlan::Stay, "unanswered");
        assert_eq!(
            revert_plan(Some(&live), Some(RevertPrompt::Revert)),
            RevertPlan::Revert
        );
        for nothing in [None, Some(&saved), Some(&folder)] {
            assert_eq!(
                revert_plan(nothing, Some(RevertPrompt::Revert)),
                RevertPlan::Nothing,
                "{nothing:?}"
            );
        }
    }

    #[test]
    fn a_window_size_is_recorded_in_points() {
        assert_eq!(
            window_of(900.0, 600.5),
            Some(Window {
                width: 900.0,
                height: 600.5
            })
        );
        assert_eq!(window_of(0.0, 600.0), None);
        assert_eq!(window_of(f64::NAN, 600.0), None);
    }
}
