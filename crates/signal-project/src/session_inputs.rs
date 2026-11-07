//! Live input in sessions (ADR 0038 §9, `docs/session-format.md` §4b): what
//! each capture side's source read, recorded so that a replay feeds the
//! same words back and is bit-exact. [`InputHistory`] drains a run's
//! [`InputRecord`]s on the control thread; [`SessionInput`] is one side's
//! record, written as `[[inputs]]` and `inputs/<slot>.f32`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use signal_core::{Direction, InstanceAddress, PortShape, fnv1a64_words};
use signal_graph::ExecutionPlan;
use signal_output::inputs::{InputRecord, RecordedInput, RunInput};

use crate::Diagnostic;

/// The folder of recorded inputs in a session directory.
pub const INPUTS_DIR: &str = "inputs";

/// One capture side's record in a session.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionInput {
    /// The source instance.
    pub address: InstanceAddress,
    /// Its channels.
    pub channels: usize,
    /// The frames recorded, from frame 0.
    pub frames: u64,
    /// What the engine read, interleaved: f32 widened to f64 as it was.
    /// `None`: the run captured nothing for it (the null driver, no
    /// device), so it read 0 throughout, and no file is written.
    pub words: Option<Vec<f32>>,
    /// Whether the record holds every frame the run played.
    pub complete: bool,
    /// Why it is incomplete.
    pub reason: Option<String>,
    /// Provenance: the device and channels it captured from.
    pub device: Option<InputDevice>,
}

/// The device a capture side read, as provenance (never used by replay).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDevice {
    /// The device's name.
    pub name: String,
    /// Its input channels, from 1, per port (0: none, read as 0).
    pub channels: Vec<u16>,
}

/// The file name of a capture side at `address`: its path, dots between
/// the names (`/mic` is `mic`, `/band/mic` is `band.mic`).
pub fn slot_name(address: &InstanceAddress) -> String {
    address
        .to_string()
        .trim_start_matches('/')
        .replace('/', ".")
}

/// `inputs/<slot>.f32`, relative to the session directory.
pub fn input_file(address: &InstanceAddress) -> String {
    format!("{INPUTS_DIR}/{}.f32", slot_name(address))
}

/// Every source of `plan` with its channels (its signal output rows), as
/// the engine and the capture sides count them.
pub fn plan_sources(plan: &ExecutionPlan) -> Vec<(InstanceAddress, usize)> {
    plan.instances
        .iter()
        .filter(|i| i.descriptor.source.is_some())
        .map(|i| {
            let width = i
                .descriptor
                .ports
                .iter()
                .filter(|p| p.direction == Direction::Out && p.shape == PortShape::Signal)
                .count();
            (i.address.clone(), width)
        })
        .collect()
}

/// The sources of every plan, each once, in order of appearance.
pub fn sources_of(plans: &[ExecutionPlan]) -> Vec<(InstanceAddress, usize)> {
    let mut seen = BTreeSet::new();
    plans
        .iter()
        .flat_map(plan_sources)
        .filter(|(address, _)| seen.insert(address.clone()))
        .collect()
}

/// A run's input records, drained on the control thread as the run goes
/// (the record rings are bounded) and turned into [`SessionInput`]s when
/// it ends.
pub struct InputHistory {
    sides: Vec<Side>,
}

struct Side {
    record: InputRecord,
    device: Option<InputDevice>,
    words: Vec<f32>,
}

impl InputHistory {
    /// Over `records`, each with the device it captured from. A capture
    /// side with no record is one the run captured nothing for: it read 0.
    pub fn new(records: Vec<(InputRecord, Option<InputDevice>)>) -> InputHistory {
        InputHistory {
            sides: records
                .into_iter()
                .map(|(record, device)| Side {
                    record,
                    device,
                    words: Vec::new(),
                })
                .collect(),
        }
    }

    /// Takes what the rings hold now.
    pub fn drain(&mut self) {
        for side in &mut self.sides {
            side.record.drain(&mut side.words);
        }
    }

    /// The records of `sources` (a plan's, [`plan_sources`]) for a run that
    /// played `frames` frames: each side's words, complete when they are
    /// every frame and nothing overflowed; a source with no record read 0.
    pub fn finish(
        mut self,
        sources: &[(InstanceAddress, usize)],
        frames: u64,
    ) -> Vec<SessionInput> {
        self.drain();
        sources
            .iter()
            .map(|(address, channels)| {
                let found = self
                    .sides
                    .iter()
                    .position(|s| s.record.address == *address && s.record.width == *channels);
                let Some(side) = found.map(|i| self.sides.swap_remove(i)) else {
                    return SessionInput {
                        address: address.clone(),
                        channels: *channels,
                        frames,
                        words: None,
                        complete: true,
                        reason: None,
                        device: None,
                    };
                };
                let width = (*channels).max(1);
                let mut words = side.words;
                words.truncate(words.len() / width * width);
                let recorded = (words.len() / width) as u64;
                let overflows = side
                    .record
                    .counts()
                    .record_overflows
                    .load(std::sync::atomic::Ordering::Relaxed);
                let reason = if overflows > 0 {
                    Some(format!(
                        "the recorder fell behind: {overflows} samples were lost (a full ring)"
                    ))
                } else if recorded != frames {
                    Some(format!(
                        "the record holds {recorded} frames of the run's {frames}"
                    ))
                } else {
                    None
                };
                SessionInput {
                    address: address.clone(),
                    channels: *channels,
                    frames: recorded,
                    words: Some(words),
                    complete: reason.is_none(),
                    reason,
                    device: side.device,
                }
            })
            .collect()
    }
}

/// `[[inputs]]` of `session.toml`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputToml {
    slot: String,
    address: String,
    channels: u64,
    frames: u64,
    complete: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    zeros: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fnv1a64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device: Option<InputDevice>,
}

impl InputToml {
    pub(crate) fn of(input: &SessionInput) -> InputToml {
        InputToml {
            slot: slot_name(&input.address),
            address: input.address.to_string(),
            channels: input.channels as u64,
            frames: input.frames,
            complete: input.complete,
            zeros: input.words.is_none(),
            reason: input.reason.clone(),
            fnv1a64: input
                .words
                .as_ref()
                .map(|w| format!("{:016x}", fnv1a64_words(w))),
            device: input.device.clone(),
        }
    }
}

/// The bytes of `inputs/<slot>.f32`: the words, f32 little-endian.
pub(crate) fn input_bytes(words: &[f32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// `[[inputs]]` checked, each with its file read by `read` (relative to the
/// session directory): the slot is the address's, every address once, a
/// recorded side's file holds `frames × channels` words with its hash, a
/// `zeros` one has no file. Diagnostics are `session.toml`'s, at `offset`
/// of each table.
pub(crate) fn load_inputs(
    tables: &[toml::Spanned<InputToml>],
    mut read: impl FnMut(&str) -> Result<Vec<u8>, crate::LoadError>,
    at: impl Fn(usize, String) -> Diagnostic,
) -> Result<Vec<SessionInput>, crate::LoadError> {
    let mut inputs = Vec::new();
    let mut diagnostics = Vec::new();
    let mut seen = BTreeSet::new();
    for table in tables {
        let offset = table.span().start;
        let table = table.get_ref();
        let mut fail = |message: String| diagnostics.push(at(offset, message));
        let Ok(address) = table.address.parse::<InstanceAddress>() else {
            fail(format!(
                "input address `{}` is not an instance address",
                table.address
            ));
            continue;
        };
        if table.slot != slot_name(&address) {
            fail(format!(
                "input slot `{}` is not its address's (`{}`)",
                table.slot,
                slot_name(&address)
            ));
            continue;
        }
        if !seen.insert(address.clone()) {
            fail(format!("input {address} is recorded twice"));
            continue;
        }
        let Ok(channels) = usize::try_from(table.channels) else {
            fail(format!("input {address}: {} channels", table.channels));
            continue;
        };
        if channels == 0 {
            fail(format!("input {address} has no channels"));
            continue;
        }
        if table.complete == table.reason.is_some() {
            fail(format!(
                "input {address}: `reason` is given exactly when it is not complete"
            ));
            continue;
        }
        let words = match (table.zeros, &table.fnv1a64) {
            (true, None) => None,
            (true, Some(_)) => {
                fail(format!("input {address}: a `zeros` input has no `fnv1a64`"));
                continue;
            }
            (false, None) => {
                fail(format!("input {address} has no `fnv1a64`"));
                continue;
            }
            (false, Some(hash)) => {
                let file = input_file(&address);
                let bytes = read(&file)?;
                let expected = table
                    .frames
                    .checked_mul(channels as u64)
                    .and_then(|w| w.checked_mul(4));
                if expected != Some(bytes.len() as u64) {
                    fail(format!(
                        "input {address}: {file} holds {} bytes, not {} frames of {channels} f32 channels",
                        bytes.len(),
                        table.frames
                    ));
                    continue;
                }
                let words: Vec<f32> = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect();
                let found = format!("{:016x}", fnv1a64_words(&words));
                if found != *hash {
                    fail(format!(
                        "input {address}: {file} has FNV-1a 64 {found}; `fnv1a64` records {hash}"
                    ));
                    continue;
                }
                Some(words)
            }
        };
        inputs.push(SessionInput {
            address,
            channels,
            frames: table.frames,
            words,
            complete: table.complete,
            reason: table.reason.clone(),
            device: table.device.clone(),
        });
    }
    if diagnostics.is_empty() {
        Ok(inputs)
    } else {
        Err(crate::LoadError::Invalid(diagnostics))
    }
}

/// The replay's [`RunInput`] of `input`, for blocks of up to `block`
/// frames: its recorded words (a `zeros` one delivers none, so reads 0).
pub(crate) fn replay_input(input: &SessionInput, block: usize) -> RunInput {
    let words = input.words.clone().unwrap_or_default();
    let (run, _) = RunInput::new(
        input.address.clone(),
        input.channels,
        Box::new(RecordedInput::new(words, input.channels)),
        block,
        None,
    );
    run
}
