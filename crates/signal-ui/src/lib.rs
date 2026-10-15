//! Signal's UI protocol and local server (ADR 0023, `docs/ui-protocol.md`).
//! It runs on the control thread: the audio thread is reached only through
//! the run's `Controller` and its queues.
//!
//! - [`Host`]: the protocol's state, stepped by its owner's control loop:
//!   the run, the clients, their subscriptions and edits.
//! - The servers: static files over HTTP (a small `std::net` server with a
//!   fixed pool of workers and deadlines: the repository's `ui/` and the
//!   project's `ui/`) and the protocol over a WebSocket
//!   (`tungstenite`), on 127.0.0.1 unless remote clients are allowed
//!   explicitly; every client needs the session token.
//! - [`panel`], [`setup`] and [`player`]: the parameter panel, the Setup
//!   file and how a run opens its devices, shared with Signal.app (M2.5).
//! - [`bindings`]: the binding check of a project's panels (`signal ui
//!   check`, and the protocol's `panel.check` when a panel loads).
//! - [`frames`]: the binary data frames.
//! - [`app`]: Signal.app's rules, shared by its `AppState` and the Host's
//!   app mode ([`Host::new_app`], M2.8-4).

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod app;
pub mod bindings;
mod entities;
pub mod frames;
mod host;
mod http;
#[cfg(feature = "test-hooks")]
mod local;
pub mod panel;
pub mod player;
mod server;
pub mod setup;

use std::path::{Path, PathBuf};

pub use host::{AppHostConfig, Finished, FirstRun, Host, HostConfig, project_ui};
#[cfg(feature = "test-hooks")]
pub use local::LocalStream;
#[cfg(feature = "test-hooks")]
pub use server::LocalConnector;
pub use server::ServerOptions;

/// The protocol version (`hello`, `welcome`, byte 1 of every binary frame).
pub const PROTOCOL: u32 = 1;

/// The page of Signal.app's window, in the UI library (`ui/app/`).
pub const APP_PAGE: &str = "/ui/app/index.html";

/// The UI library to serve: the app bundle's own (`Contents/Resources/ui`,
/// beside `Contents/MacOS` and `Contents/Helpers`, where `exe` runs from),
/// else `built`, the repository's `ui/` the program was built from.
pub fn library_dir(exe: Option<&Path>, built: &Path) -> PathBuf {
    let bundled = exe
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|contents| contents.join("Resources/ui"));
    match bundled {
        Some(dir) if dir.join("signal-ui.js").is_file() => dir,
        _ => built.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::library_dir;
    use std::fs;
    use std::path::Path;

    #[test]
    fn the_bundle_s_ui_library_is_preferred() {
        let base = std::env::temp_dir().join(format!("signal-ui-library-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let contents = base.join("Signal.app/Contents");
        let exe = contents.join("MacOS/Signal");
        let built = Path::new("/repo/ui");
        assert_eq!(library_dir(Some(&exe), built), built, "no bundled ui/");
        fs::create_dir_all(contents.join("Resources/ui")).unwrap();
        fs::write(contents.join("Resources/ui/signal-ui.js"), "").unwrap();
        assert_eq!(
            library_dir(Some(&exe), built),
            contents.join("Resources/ui")
        );
        let helper = contents.join("Helpers/signal");
        assert_eq!(
            library_dir(Some(&helper), built),
            contents.join("Resources/ui")
        );
        assert_eq!(library_dir(None, built), built);
        fs::remove_dir_all(&base).unwrap();
    }
}
