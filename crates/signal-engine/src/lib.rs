//! Signal engine: the per-frame executor (ADR 0016 frame phases, event order,
//! parameter bases, ramps, suspension), fault guards, and the offline and
//! real-time drivers. The offline driver is the reference (ADR 0012).
//! Real-time code follows docs/realtime-rules.md. The semantics it implements
//! are normative in `docs/semantics.md`.
//!
//! - [`Engine`]: instantiates an [`ExecutionPlan`](signal_graph::ExecutionPlan)
//!   through a [`Registry`](signal_core::Registry) and runs it in blocks of
//!   *k* frames (ADR 0019 §1), bit-identical to one frame at a time.
//!   [`Engine::run_block`] and [`Engine::run_frame`] allocate nothing.
//! - Live edits (ADR 0006): [`Engine::prepare_swap`] and [`Engine::commit_swap`]
//!   replace the plan at an exact frame, moving each surviving instance's state;
//!   [`Engine::push_external`], [`Engine::push_suspend`] and
//!   [`Engine::push_resume`] queue input while the engine runs, and
//!   [`Engine::drain_log`] takes each frame's log out. None of them but
//!   `prepare_swap` allocates.
//! - The sink handoff (ADR 0022): the engine knows a sink only as an
//!   instance whose descriptor declares a [`SinkKind`](signal_core::SinkKind)
//!   ([`Engine::sinks`]). Both drivers hand each block's sink inputs
//!   ([`Engine::sink_input_at`]) to a [`SinkOutput`] the caller supplies;
//!   which sinks it serves and what their inputs become (a device, a file,
//!   their gain and conversion) is the output adapter's policy, outside the
//!   engine (`signal-output` for audio).
//! - [`run_offline`]: the offline driver. It runs a fresh engine in blocks
//!   and records the event log; `before` lets the caller swap plans and push
//!   input before the frames it names (how `signal replay` applies a
//!   recorded session). The render artifacts' [`Manifest`] is defined here.
//! - [`realtime`]: the real-time [`Runner`], called from a driver's audio
//!   callback, and the [`Controller`] that feeds it from the control thread
//!   through `rtrb` queues ([`Controller::poll`] drains logs, parameter
//!   values and command reports into a [`ControlSink`], and reports a hung
//!   callback, M2.7-9). [`Runner::process`] allocates nothing.
//! - Failures (`docs/semantics.md` §14): a primitive's panic mutes its
//!   instance (a `panic` fault); anything else that fails stops the engine
//!   ([`Engine::failure`]). Test-only hooks need the feature `test-hooks`.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

// The failure policy catches unwinds (`docs/semantics.md` §14).
#[cfg(panic = "abort")]
compile_error!("signal-engine needs panic = \"unwind\": a module's panic must be caught");

mod board;
mod cores;
mod engine;
mod error;
mod fade;
mod gate;
mod log;
mod manifest;
#[cfg(all(test, loom))]
mod model;
mod offline;
mod parameter;
mod park;
mod pool;
mod realtime;
#[doc(hidden)]
pub mod shared;
mod sink;
mod sync;
mod tasks;
mod units;
mod wake;

pub use cores::{
    CoreSource, PerfLevel, cpu_levels, cpu_lines, performance_cores, realtime_threads,
};
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use engine::Injection;
pub use engine::{
    EditFull, Engine, EngineConfig, ExternalEvent, ExternalFull, InputDiscards, MAX_PREPARED_BYTES,
    MAX_RESERVED_BYTES, ParameterValue, PreparedSwap, Scripted,
};
#[doc(hidden)]
pub use engine::{Schedule, ScheduleStats};
pub use error::EngineError;
pub use log::{
    DiscardReason, EngineFailure, FailureReason, FaultReason, InstanceId, LineSource, LogLine,
    LogRecord, LogSource, PortId,
};
pub use manifest::{ComputeDevice, Manifest, Placement, Provenance, SameMachine};
pub use offline::{Offline, OfflineError, run_offline};
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use pool::spawned_workers;
pub use pool::{Progress, Workers};
pub use realtime::{
    BoundaryLine, BoundaryOutcome, Command, CommandId, ControlError, ControlSink, Controller,
    HangLine, Outcome, PARAMETERS_PER_CALLBACK, ReadySwap, RealtimeConfig, Report, Runner,
    RunnerStatus, SentCommand, SinksChanged, Stamp, realtime, realtime_with,
};
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use shared::{Exclusivity, HookPoint, exclusivity};
pub use sink::{Sink, SinkOutput};
pub use wake::{Computation, MakeWaiter, RtOptions, RtPolicy, Spread, Waiter, WakeStats};
