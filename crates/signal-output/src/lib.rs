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
//!
//! The scope adapter (`scope`, M2.8-1) captures any signal output or signal
//! input of the running plan, decimated, for display; it reads and never
//! changes them. The vector adapter (`vectors`, M2.8-7) snapshots vector
//! outputs (the visual-rate models' state) for `<signal-canvas>`, in real
//! time and, for a replay, offline ([`render_blocks_tapped`]).

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
mod scope;
mod vectors;

pub use audio::{Conversion, Meter, RenderError, device_sample};
pub use driver::{CallbackSizes, Driver, DriverError, MAX_CALLBACK_FRAMES, NullDriver};
pub use render::{Render, render, render_blocks, render_blocks_tapped, render_with};
pub use runner::{Controller, ReadySwap, Runner, realtime, realtime_with};
pub use scope::{SCOPE_POINTS, SCOPE_SLOTS, ScopeBlock, ScopeError};
pub use vectors::{
    VECTOR_MAX_ROWS, VECTOR_SLOTS, VectorError, VectorSnapshot, VectorTap, VectorTrack,
    VectorUnbound, vector_shape,
};
