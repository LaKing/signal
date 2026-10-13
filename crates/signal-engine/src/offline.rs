//! The offline driver, the reference implementation (ADR 0012): runs N frames
//! in blocks, hands each block's sink inputs to a [`SinkOutput`] (ADR 0022)
//! and records the event log. Not real-time code: it allocates;
//! [`Engine::run_block`] does not. What becomes of the sinks' inputs (an
//! audio file, its gain and conversion) is the output adapter's
//! (`signal-output`).

use std::fmt;

use signal_core::Frame;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::log::EngineFailure;
use crate::manifest::SameMachine;
use crate::sink::SinkOutput;

/// What [`run_offline`] kept besides what its output recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct Offline {
    /// The sample rate in frames per second.
    pub sample_rate: u32,
    /// The master seed.
    pub seed: u64,
    /// The number of frames run.
    pub frames: u64,
    /// The bytes of `log.jsonl`.
    pub log: Vec<u8>,
    /// The instances of any of its plans that are reproducible on the same
    /// machine only (ADR 0019 §3), by address; empty for a bit-exact run.
    pub same_machine: Vec<SameMachine>,
}

/// Why [`run_offline`] stopped.
#[derive(Debug)]
pub enum OfflineError {
    /// The engine has already run frames; a run starts at frame 0.
    AlreadyStarted,
    /// The output did not find its sinks in the plan
    /// ([`SinkOutput::bind`]), at the start or after a swap.
    Unbound,
    /// The caller's step before a frame failed: a swap or an edit was
    /// refused.
    Edit(EngineError),
    /// The engine failed and stopped (`docs/semantics.md` §14): nothing of
    /// the run is kept.
    EngineFailed(EngineFailure),
    /// Serializing a log line failed.
    Json(serde_json::Error),
}

impl fmt::Display for OfflineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OfflineError::AlreadyStarted => f.write_str("a render must start at frame 0"),
            OfflineError::Unbound => f.write_str("the output's sinks are not in the plan"),
            OfflineError::Edit(error) => write!(f, "applying an edit: {error}"),
            OfflineError::EngineFailed(failure) => write!(f, "{failure}"),
            OfflineError::Json(error) => write!(f, "serializing render artifacts: {error}"),
        }
    }
}

impl std::error::Error for OfflineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            OfflineError::Json(error) => Some(error),
            _ => None,
        }
    }
}

/// Runs `frames` frames of a fresh `engine` in blocks of at most `block`
/// frames (and the engine's block capacity), handing each block's sink
/// inputs to `output`. It calls `before` before the first block and again
/// before the block starting at the frame it returns, so that it can commit
/// a swap or push input there. `before` returns the next frame it must run
/// before, or `None` for none: a block never spans it, so a swap always
/// falls on a block's first frame. Input it pushes for later frames lands at
/// those frames inside a block. After a swap the output binds again
/// ([`SinkOutput::bind`]). Each frame's log records go to `log.jsonl` in
/// canonical order: a swap, then edits, then events and discards by target
/// port address, then faults; each block's log is drained after it is
/// recorded.
///
/// The result does not depend on the block size (`docs/semantics.md` §13,
/// with §14's exception).
pub fn run_offline(
    engine: &mut Engine,
    frames: u64,
    block: usize,
    mut before: impl FnMut(&mut Engine) -> Result<Option<Frame>, EngineError>,
    output: &mut impl SinkOutput,
) -> Result<Offline, OfflineError> {
    if engine.next_frame() != Frame::ZERO {
        return Err(OfflineError::AlreadyStarted);
    }
    if !output.bind(engine) {
        return Err(OfflineError::Unbound);
    }
    let mut log = Vec::new();
    let mut records = Vec::new();
    let mut same_machine = engine.same_machine();
    let mut version = engine.version();
    let mut call = Some(engine.next_frame());
    let mut done = 0u64;
    while done < frames {
        let next = engine.next_frame();
        if call.is_some_and(|at| at <= next) {
            call = before(engine).map_err(OfflineError::Edit)?;
        }
        if engine.version() != version {
            version = engine.version();
            if !output.bind(engine) {
                return Err(OfflineError::Unbound);
            }
            same_machine.extend(engine.same_machine());
        }
        let until_call = call.map_or(u64::MAX, |at| at.0.saturating_sub(next.0).max(1));
        let len = (frames - done).min(until_call);
        let len = usize::try_from(len)
            .map_or(block, |len| len.min(block))
            .max(1);
        let ran = engine.run_block(len);
        if let Some(failure) = engine.failure() {
            return Err(OfflineError::EngineFailed(failure));
        }
        if ran == 0 {
            break;
        }
        done += ran as u64;
        output.block(engine, ran);
        records.clear();
        records.extend_from_slice(engine.frame_log());
        records.sort_by_key(|record| (record.frame(), record.order()));
        for record in &records {
            if let Some(line) = engine.log_line(record) {
                serde_json::to_writer(&mut log, &line).map_err(OfflineError::Json)?;
                log.push(b'\n');
            }
        }
        engine.drain_log(&mut |_| {});
    }

    same_machine.sort();
    same_machine.dedup();
    let config = engine.config();
    Ok(Offline {
        sample_rate: config.sample_rate,
        seed: config.seed,
        frames,
        log,
        same_machine,
    })
}
