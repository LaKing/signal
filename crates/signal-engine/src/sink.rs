//! The sink handoff (ADR 0022): the engine knows sinks only as instances
//! whose descriptor declares a [`SinkKind`], and hands their inputs, block by
//! block, to a [`SinkOutput`] that a driver supplies. What a kind means (which
//! sink to play, its parameters, how its inputs reach a device or a file) is
//! the output adapter's policy, outside the engine (`signal-output`).

use signal_core::{InstanceAddress, SinkKind};

use crate::engine::Engine;
use crate::log::InstanceId;

/// A sink of the running plan ([`Engine::sinks`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sink<'a> {
    /// The instance, for [`Engine::sink_input_at`].
    pub id: InstanceId,
    /// The kind its descriptor declares.
    pub kind: SinkKind,
    /// Its address.
    pub address: &'a InstanceAddress,
    /// Its number of signal inputs.
    pub inputs: usize,
}

/// What a driver hands a run's sink inputs to: the offline driver
/// ([`run_offline`](crate::run_offline)) and the real-time
/// [`Runner`](crate::Runner) call it after each block. In real time both
/// methods run on the audio thread and must allocate, lock and block
/// nowhere (`docs/realtime-rules.md`).
pub trait SinkOutput {
    /// Finds the sinks it serves in the running plan ([`Engine::sinks`]):
    /// before the first block, and after every swap, at the swap's frame.
    /// False if the plan no longer has them; the offline driver then stops
    /// ([`OfflineError::Unbound`](crate::OfflineError::Unbound)).
    fn bind(&mut self, engine: &Engine) -> bool;

    /// The block just run, of `frames` frames: its sinks' inputs are
    /// [`Engine::sink_input_at`] for frames `0..frames`, and the effective
    /// parameter values [`Engine::parameter_at_frame`].
    fn block(&mut self, engine: &Engine, frames: usize);
}
