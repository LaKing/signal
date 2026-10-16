//! What the window decides without a window: its native menu bar (as data;
//! `muda` builds it on the main thread), what each of Signal's own items
//! does, where the WebView may navigate, and the size recorded in the Setup;
//! and for `.signal` projects (M2.8-12): what a drop or a choice opens, the
//! window's title, Save As's default name, and what the "Save changes?"
//! prompt offers and leads to on each way of quitting.

use std::path::{Path, PathBuf};

use signal_project::archive;
use signal_ui::ProjectStatus;

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
    /// Interface Setup…: the page shows the Setup tab.
    Setup,
    /// Quit: the window closes as with its close button (the Setup records
    /// its size).
    Quit,
}

impl MenuAction {
    const ALL: [MenuAction; 5] = [
        MenuAction::Open,
        MenuAction::Save,
        MenuAction::SaveAs,
        MenuAction::Setup,
        MenuAction::Quit,
    ];

    /// The menu item's id.
    pub fn id(self) -> &'static str {
        match self {
            MenuAction::Open => "open",
            MenuAction::Save => "save",
            MenuAction::SaveAs => "save-as",
            MenuAction::Setup => "setup",
            MenuAction::Quit => "quit",
        }
    }

    /// The action of the menu item `id`, if it is one of Signal's own.
    pub fn of(id: &str) -> Option<MenuAction> {
        MenuAction::ALL.into_iter().find(|a| a.id() == id)
    }

    /// Whether its Cmd-key takes Shift too (Save As…, ⇧⌘S).
    pub fn shift(self) -> bool {
        self == MenuAction::SaveAs
    }

    /// The script that hands the action to the page (a `signal-menu`
    /// event, `ui/app/app.js`); none for Save As and Quit, which the window
    /// does.
    pub fn page_script(self) -> Option<String> {
        (!matches!(self, MenuAction::Quit | MenuAction::SaveAs)).then(|| {
            let quoted = serde_json::to_string(self.id()).unwrap_or_else(|_| "\"\"".to_owned());
            format!("window.dispatchEvent(new CustomEvent('signal-menu', {{ detail: {quoted} }}))")
        })
    }
}

/// An item of the menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    /// One of Signal's own: its action, label and Cmd-key.
    Own(MenuAction, &'static str, char),
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
    /// A line between items.
    Separator,
}

const QUIT: MenuItem = MenuItem::Own(MenuAction::Quit, "Quit Signal", 'q');

/// The menu bar: the app menu, File and Edit, in order (ADR 0023).
pub const MENU_BAR: [(&str, &[MenuItem]); 3] = [
    (
        "Signal",
        &[
            MenuItem::About,
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
            MenuItem::Own(MenuAction::Open, "Open…", 'o'),
            MenuItem::Own(MenuAction::Save, "Save", 's'),
            MenuItem::Own(MenuAction::SaveAs, "Save As…", 's'),
            MenuItem::Separator,
            MenuItem::Own(MenuAction::Setup, "Interface Setup…", ','),
            MenuItem::Separator,
            MenuItem::Own(MenuAction::Quit, "Quit", 'q'),
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
];

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

/// The window's size in points, as the Setup records it.
pub fn window_of(width: f64, height: f64) -> Option<Window> {
    (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0).then_some(Window {
        width: width as f32,
        height: height as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own(menu: &str) -> Vec<(&'static str, char)> {
        MENU_BAR
            .iter()
            .find(|(name, _)| *name == menu)
            .unwrap()
            .1
            .iter()
            .filter_map(|item| match item {
                MenuItem::Own(_, label, key) => Some((*label, *key)),
                _ => None,
            })
            .collect()
    }

    /// M2.8-11 AC12, AC15: the app menu, File and Edit.
    #[test]
    fn the_menu_bar_has_the_app_menu_file_and_edit() {
        let names: Vec<&str> = MENU_BAR.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["Signal", "File", "Edit"]);
        assert_eq!(
            own("File"),
            [
                ("Open…", 'o'),
                ("Save", 's'),
                ("Save As…", 's'),
                ("Interface Setup…", ','),
                ("Quit", 'q')
            ]
        );
        let shifted: Vec<MenuAction> = MenuAction::ALL.into_iter().filter(|a| a.shift()).collect();
        assert_eq!(shifted, [MenuAction::SaveAs], "only Save As is ⇧⌘S");
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

    #[test]
    fn each_own_item_has_its_action_and_the_page_gets_all_but_quit() {
        for (_, items) in MENU_BAR {
            for item in items {
                if let MenuItem::Own(action, ..) = item {
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
