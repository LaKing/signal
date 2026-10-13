//! The reproducibility manifest of a render (ADR 0008), written as
//! `manifest.json`. Format: `docs/semantics.md`, "Artifacts".

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What the caller knows about the build and the project; the engine cannot.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The Signal source revision (e.g. a git commit).
    pub signal_revision: String,
    /// Whether the source had uncommitted changes; `None` when that could not
    /// be found out. Written as `true`, `false` or `"unknown"`.
    #[serde(with = "dirty")]
    pub signal_dirty: Option<bool>,
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
    /// The instances, in any plan of the render, whose type is reproducible
    /// on the same machine and build only (ADR 0019 §3), by address. Absent
    /// when there are none: the render is then bit-exact on any machine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub same_machine: Vec<SameMachine>,
    /// An ideal render of a session (`signal replay --ideal`, M2.7-4): every
    /// result of a latent module was awaited, so it is not what was played.
    /// Absent unless true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ideal: bool,
    /// The exact replay of a session with boundary records (M2.7-4): the
    /// audio is stored as the device received it, clamped to [−1, 1] and
    /// non-finite samples 0. Absent unless true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub device_conversion: bool,
    /// An exact replay run with `--trust-provenance` although the session
    /// was recorded by another build or machine (M2.7-8): its recomputed
    /// results were still checked against their hashes. Absent unless true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub trust_provenance: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// An instance whose output is reproducible on the same machine only.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SameMachine {
    /// Its address.
    pub instance: String,
    /// Its module type.
    #[serde(rename = "type")]
    pub type_name: String,
    /// Its declared latency *L*, in blocks of its own, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_blocks: Option<u32>,
    /// The size of those blocks in frames: part of the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_frames: Option<u32>,
}

/// `signal_dirty` as `true`, `false` or `"unknown"`.
mod dirty {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    const UNKNOWN: &str = "unknown";

    pub(super) fn serialize<S: Serializer>(
        value: &Option<bool>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(dirty) => serializer.serialize_bool(*dirty),
            None => serializer.serialize_str(UNKNOWN),
        }
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Known(bool),
        Text(String),
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<bool>, D::Error> {
        match Repr::deserialize(deserializer)? {
            Repr::Known(dirty) => Ok(Some(dirty)),
            Repr::Text(text) if text == UNKNOWN => Ok(None),
            Repr::Text(text) => Err(D::Error::custom(format!(
                "signal_dirty is `{text}`; expected true, false or \"unknown\""
            ))),
        }
    }
}
