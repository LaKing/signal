//! Signal's output adapters (ADR 0022). The engine hands every sink's inputs
//! to a driver generically ([`signal_engine::SinkOutput`]); an adapter here
//! decides what a kind of sink means. The audio adapter:
//!
//! - chooses the audio sink: the plan's one instance whose descriptor
//!   declares [`SinkKind::Audio`](signal_core::SinkKind::Audio), with at
//!   least one signal input;
//! - applies the sink's `gain` parameter, if its type declares one, to every
//!   input, frame by frame (`docs/semantics.md` §8);
//! - maps sink input *i* to device channel *i* (extra device channels 0);
//! - converts to the device's `f32` ([`device_sample`]: clamped to [−1, 1],
//!   non-finite as 0, counted as clipped) and meters each channel per
//!   callback.
//!
//! [`render`] (the offline WAV writer, [`Render::write`]) and the real-time
//! [`Runner`] (any [`Driver`]: the [`NullDriver`] here, CoreAudio in
//! `signal-audio`) use the same adapter.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod audio;
mod driver;
mod render;
mod runner;

pub use audio::{Conversion, Meter, RenderError, device_sample};
pub use driver::{CallbackSizes, Driver, DriverError, MAX_CALLBACK_FRAMES, NullDriver};
pub use render::{Render, render, render_blocks, render_with};
pub use runner::{Controller, Runner, realtime, realtime_with};
