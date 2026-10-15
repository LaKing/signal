//! Signal.app (ADR 0013, 0023): a tao window with a wry WebView on the
//! app's own UI server. The app's logic is plain Rust, testable without a
//! window:
//!
//! - [`setup`]: the machine Setup file (`docs/project-format.md`, "Setup"),
//!   from `signal-ui`, which the UI protocol shares (M2.8-1).
//! - [`meter`]: meter ballistics from the audio thread's per-block peak and
//!   sum of squares.
//! - [`state`]: the app's state, its [`Command`](state::Command)s and its
//!   [`View`](state::View) snapshots, playing through `signal-audio` (or the
//!   null driver) with `signal-project`'s live reload. Its rules are
//!   `signal_ui::app`'s, which the server in the app runs too.
//! - [`panel`]: the parameter panel's instances and parameters, with the
//!   runner's effective values (from `signal-ui`); its edits go through [`state`].
//! - [`dialog`]: the interface dialog's draft, which keeps chosen devices
//!   by identity.
//! - [`server`]: the server in the app: the control thread, which runs the
//!   UI protocol's host in app mode; the window's page is its client.
//! - [`window`]: what the window decides without a window (its native menu
//!   bar and what each item does, where it may navigate, the size the Setup
//!   records).

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod dialog;
pub mod meter;
pub use signal_ui::{panel, setup};
pub mod server;
pub mod state;
pub mod window;
