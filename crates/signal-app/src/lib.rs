//! Signal.app (ADR 0013, 0018): the GUI shell over the engine. The app's
//! logic is plain Rust, testable without a window:
//!
//! - [`setup`]: the machine Setup file (`docs/project-format.md`, "Setup"),
//!   from `signal-ui`, which the UI protocol shares (M2.8-1).
//! - [`meter`]: meter ballistics from the audio thread's per-block peak and
//!   sum of squares.
//! - [`state`]: the app's state, its [`Command`](state::Command)s and its
//!   [`View`](state::View) snapshots, playing through `signal-audio` (or the
//!   null driver) with `signal-project`'s live reload.
//! - [`panel`]: the parameter panel's instances and parameters, with the
//!   runner's effective values (from `signal-ui`); its edits go through [`state`].
//! - [`dialog`]: the interface dialog's draft, which keeps chosen devices
//!   by identity.
//! - [`control`]: the control thread that owns the state; `rtrb` queues to
//!   the window.
//! - [`ui`]: the egui window, which only draws views and sends commands.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod control;
pub mod dialog;
pub mod meter;
pub use signal_ui::{panel, setup};
pub mod state;
pub mod ui;
