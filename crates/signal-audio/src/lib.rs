//! Signal's audio device driver (ADR 0011): CoreAudio through `cpal`, pinned and
//! isolated in this crate behind the engine's driver trait.
//!
//! - [`CoreAudioDriver`]: the [`Driver`](signal_engine::Driver) of an output
//!   device, opened at the project's sample rate (no resampling) with a fixed
//!   buffer; [`devices`] lists output and input devices, with their stable
//!   ids; [`InputCapture`] runs an input stream for metering only.
//! - [`labels`] and [`resolve`] tell same-named devices apart and find a
//!   saved one again (by its id, never by position).
//! - Everything but the cpal glue (`coreaudio`) is plain code tested without a
//!   device: [`choose_output`] picks a configuration, [`ChannelMap`] maps the
//!   sink's channels to device channels, [`OutputCallback`] is the body of the
//!   audio callback, [`InputCallback`] that of the input callback, and
//!   [`CallbackStats`] measures them (statistics only, never
//!   semantics). The callback allocates, frees, locks and blocks nowhere
//!   (`docs/realtime-rules.md`).
//! - Real-time workers (ADR 0019 §2, §4): when the stream starts, the driver
//!   puts each of the engine's worker threads into the Mach time-constraint
//!   policy and the output device's I/O workgroup ([`rt_thread`], the
//!   audited `unsafe` module), and takes them out when it stops;
//!   [`Scheduling`] says which path each worker took.

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
mod input;
mod map;
pub mod rt_thread;
mod scheduling;
mod select;
mod stats;

pub use callback::{CallbackControl, OutputCallback};
pub use config::{
    AudioError, Choice, ConfigRange, DeviceInfo, Direction, choose_input, choose_output,
};
pub use coreaudio::{CoreAudioDriver, DeviceSelector, InputCapture, devices};
pub use input::{InputCallback, InputMeters};
pub use map::ChannelMap;
pub use scheduling::{
    Demotions, DeviceMatch, Scheduling, TimeConstraint, WorkerPath, match_device,
};
pub use select::{DeviceRef, Resolution, labels, resolve, signature};
pub use stats::{CallbackStats, StatsSnapshot};
