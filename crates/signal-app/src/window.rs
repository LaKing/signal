//! What the window decides without a window: the page's requests to the
//! native side (wry's IPC: `window.ipc.postMessage`), where the WebView may
//! navigate, and the size recorded in the Setup.

use std::path::Path;

use crate::setup::Window;

/// A request from the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The menu's Quit (or Cmd-Q): close the window, as its close button.
    Quit,
}

/// The page's IPC message `message` as an action, if it is one.
pub fn ipc_action(message: &str) -> Option<Action> {
    match message.trim() {
        "quit" => Some(Action::Quit),
        _ => None,
    }
}

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

    #[test]
    fn the_page_asks_to_quit() {
        assert_eq!(ipc_action("quit"), Some(Action::Quit));
        assert_eq!(ipc_action("rm -rf"), None);
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
