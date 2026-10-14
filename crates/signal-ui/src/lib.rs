//! Signal's UI protocol and local server (ADR 0023, `docs/ui-protocol.md`).
//! It runs on the control thread: the audio thread is reached only through
//! the run's `Controller` and its queues.
//!
//! - [`Host`]: the protocol's state, stepped by its owner's control loop:
//!   the run, the clients, their subscriptions and edits.
//! - The servers: static files over HTTP (`tiny_http`: the repository's
//!   `ui/` and the project's `ui/`) and the protocol over a WebSocket
//!   (`tungstenite`), on 127.0.0.1 unless remote clients are allowed
//!   explicitly; every client needs the session token.
//! - [`panel`], [`setup`] and [`player`]: the parameter panel, the Setup
//!   file and how a run opens its devices, shared with Signal.app (M2.5).
//! - [`bindings`]: the binding check of a project's panels (`signal ui
//!   check`, and the protocol's `panel.check` when a panel loads).
//! - [`frames`]: the binary data frames.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod bindings;
mod entities;
pub mod frames;
mod host;
#[cfg(feature = "test-hooks")]
mod local;
pub mod panel;
pub mod player;
mod server;
pub mod setup;

pub use host::{Finished, FirstRun, Host, HostConfig, project_ui};
#[cfg(feature = "test-hooks")]
pub use local::LocalStream;
pub use server::ServerOptions;

/// The protocol version (`hello`, `welcome`, byte 1 of every binary frame).
pub const PROTOCOL: u32 = 1;
