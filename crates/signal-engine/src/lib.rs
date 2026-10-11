//! Signal engine: the per-frame executor (ADR 0016 frame phases, event order,
//! parameter bases, ramps, suspension), fault guards, and drivers. The offline
//! driver is the reference (ADR 0012). Real-time code follows docs/realtime-rules.md.
//! The semantics it implements are normative in `docs/semantics.md`.
//!
//! - [`Engine`]: instantiates an [`ExecutionPlan`](signal_graph::ExecutionPlan)
//!   through a [`Registry`](signal_core::Registry) and runs it one frame at a time
//!   (*k* = 1). [`Engine::run_frame`] allocates nothing.
//! - [`render`]: the offline driver. It records the single audio sink's inputs
//!   and the event log; [`Render::write`] writes `audio.wav`, `log.jsonl` and
//!   `manifest.json` ([`Manifest`]).
//!
//! Planned for M2: the real-time driver, plan swap and live edits.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod engine;
mod error;
mod fade;
mod log;
mod manifest;
mod offline;
mod parameter;

pub use engine::{Engine, EngineConfig, ExternalEvent, ParameterValue};
pub use error::{EngineError, RenderError};
pub use log::{
    DiscardReason, FaultReason, InstanceId, LineSource, LogLine, LogRecord, LogSource, PortId,
};
pub use manifest::{Manifest, Provenance};
pub use offline::{Render, render};
