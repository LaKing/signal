//! Signal engine: the per-frame executor (ADR 0016 frame phases, event order,
//! parameter bases, ramps, suspension), fault guards, and drivers. The offline
//! driver is the reference (ADR 0012). Real-time code follows docs/realtime-rules.md.
//! The semantics it implements are normative in `docs/semantics.md`.
//!
//! - [`Engine`]: instantiates an [`ExecutionPlan`](signal_graph::ExecutionPlan)
//!   through a [`Registry`](signal_core::Registry) and runs it one frame at a time
//!   (*k* = 1). [`Engine::run_frame`] allocates nothing.
//! - Live edits (ADR 0006): [`Engine::prepare_swap`] and [`Engine::commit_swap`]
//!   replace the plan at an exact frame, moving each surviving instance's state;
//!   [`Engine::push_external`], [`Engine::push_suspend`] and
//!   [`Engine::push_resume`] queue input while the engine runs, and
//!   [`Engine::drain_log`] takes each frame's log out. None of them but
//!   `prepare_swap` allocates.
//! - [`render`]: the offline driver. It records the single audio sink's inputs
//!   and the event log; [`Render::write`] writes `audio.wav`, `log.jsonl` and
//!   `manifest.json` ([`Manifest`]). [`render_with`] also lets the caller swap
//!   plans and push input before each frame (how `signal replay` applies a
//!   recorded session).
//! - [`realtime`]: the real-time [`Runner`], called from a driver's audio
//!   callback, and the [`Controller`] that feeds it from the control thread
//!   through `rtrb` queues ([`Controller::poll`] drains logs, meters,
//!   parameter values and command reports into a [`ControlSink`]). [`Runner::process`] allocates
//!   nothing.
//! - [`Driver`]: the driver trait (ADR 0011); [`NullDriver`] runs a Runner
//!   without hardware, with seeded [`CallbackSizes`].

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod driver;
mod engine;
mod error;
mod fade;
mod log;
mod manifest;
mod offline;
mod parameter;
mod realtime;

pub use driver::{CallbackSizes, Driver, DriverError, MAX_CALLBACK_FRAMES, NullDriver};
pub use engine::{
    EditFull, Engine, EngineConfig, ExternalEvent, ExternalFull, InputDiscards, ParameterValue,
    PreparedSwap,
};
pub use error::{EngineError, RenderError};
pub use log::{
    DiscardReason, FaultReason, InstanceId, LineSource, LogLine, LogRecord, LogSource, PortId,
};
pub use manifest::{Manifest, Provenance};
pub use offline::{Render, render, render_with};
pub use realtime::{
    Command, CommandId, ControlError, ControlSink, Controller, Meter, Outcome,
    PARAMETERS_PER_CALLBACK, ReadySwap, RealtimeConfig, Report, Runner, RunnerStatus, SentCommand,
    Stamp, realtime,
};
