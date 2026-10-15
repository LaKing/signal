//! What the window decides without a window: its native menu bar (as data;
//! `muda` builds it on the main thread), what each of Signal's own items
//! does, where the WebView may navigate, and the size recorded in the Setup.

use std::path::Path;

use crate::setup::Window;

/// What one of Signal's own menu items does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Open Project…: the page shows the Setup tab with its folder field
    /// focused (no native folder chooser: it would need direct `objc2`
    /// dependencies, which change Cargo.lock; ADR 0023).
    Open,
    /// Save Project: the page writes every unsaved parameter again.
    Save,
    /// Interface Setup…: the page shows the Setup tab.
    Setup,
    /// Quit: the window closes as with its close button (the Setup records
    /// its size).
    Quit,
}

impl MenuAction {
    const ALL: [MenuAction; 4] = [
        MenuAction::Open,
        MenuAction::Save,
        MenuAction::Setup,
        MenuAction::Quit,
    ];

    /// The menu item's id.
    pub fn id(self) -> &'static str {
        match self {
            MenuAction::Open => "open",
            MenuAction::Save => "save",
            MenuAction::Setup => "setup",
            MenuAction::Quit => "quit",
        }
    }

    /// The action of the menu item `id`, if it is one of Signal's own.
    pub fn of(id: &str) -> Option<MenuAction> {
        MenuAction::ALL.into_iter().find(|a| a.id() == id)
    }

    /// The script that hands the action to the page (a `signal-menu`
    /// event, `ui/app/app.js`); none for Quit, which the window does.
    pub fn page_script(self) -> Option<String> {
        (self != MenuAction::Quit).then(|| {
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
            MenuItem::Own(MenuAction::Open, "Open Project…", 'o'),
            MenuItem::Own(MenuAction::Save, "Save Project", 's'),
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

/// What dropping `path` on the window does: a folder is opened as the
/// project; anything else is refused with the message the window shows, and
/// opens nothing (so it never becomes the Setup's last project).
pub fn dropped(path: &Path) -> Result<&Path, &'static str> {
    if path.is_dir() {
        Ok(path)
    } else {
        Err("drop a project folder")
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
                ("Open Project…", 'o'),
                ("Save Project", 's'),
                ("Interface Setup…", ','),
                ("Quit", 'q')
            ]
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
            Err("drop a project folder")
        );
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
