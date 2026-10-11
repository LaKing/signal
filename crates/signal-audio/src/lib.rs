//! Signal's audio device driver (ADR 0011): CoreAudio through `cpal`, pinned and
//! isolated in this crate behind the engine's driver trait.
//!
//! - [`CoreAudioDriver`]: the [`Driver`](signal_engine::Driver) of an output
//!   device, opened at the project's sample rate (no resampling) with a fixed
//!   buffer; [`devices`] lists output and input devices.
//! - Everything but the cpal glue (`coreaudio`) is plain code tested without a
//!   device: [`choose_output`] picks a configuration, [`ChannelMap`] maps the
//!   sink's channels to device channels, [`OutputCallback`] is the body of the
//!   audio callback, and [`CallbackStats`] measures it (statistics only, never
//!   semantics). The callback allocates, frees, locks and blocks nowhere
//!   (`docs/realtime-rules.md`).

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod callback;
mod config;
mod coreaudio;
mod map;
mod stats;

pub use callback::{CallbackControl, OutputCallback};
pub use config::{AudioError, Choice, ConfigRange, DeviceInfo, Direction, choose_output};
pub use coreaudio::{CoreAudioDriver, devices};
pub use map::ChannelMap;
pub use stats::{CallbackStats, StatsSnapshot};
