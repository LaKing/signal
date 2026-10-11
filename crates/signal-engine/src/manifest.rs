//! The reproducibility manifest of a render (ADR 0008), written as
//! `manifest.json`. Format: `docs/semantics.md`, "Artifacts".

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What the caller knows about the build and the project; the engine cannot.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The Signal source revision (e.g. a git commit).
    pub signal_revision: String,
    /// Whether the source had uncommitted changes.
    pub signal_dirty: bool,
    /// The compiler, as `rustc --version` prints it.
    pub rustc_version: String,
    /// A hash of every project file, by path relative to the project folder.
    pub project_file_hashes: BTreeMap<String, String>,
    /// A hash of `Cargo.lock`.
    pub cargo_lock_hash: String,
}

/// Everything needed to reproduce a render and check the result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Supplied by the caller.
    pub provenance: Provenance,
    /// The master seed.
    pub seed: u64,
    /// The sample rate in frames per second.
    pub sample_rate: u32,
    /// The number of frames rendered.
    pub frames: u64,
    /// The build profile: whether debug assertions were compiled in.
    pub debug_assertions: bool,
    /// FNV-1a 64 of the bytes of `log.jsonl`, as 16 lowercase hex digits.
    pub log_fnv1a64: String,
    /// FNV-1a 64 of the audio samples as `audio.wav` stores them (32-bit float,
    /// little-endian, interleaved), as 16 lowercase hex digits.
    pub audio_fnv1a64: String,
}
