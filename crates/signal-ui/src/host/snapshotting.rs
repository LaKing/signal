//! Snapshots (ADR 0041): listing, saving, restoring and their transitions.

use serde_json::{Value, json};
use signal_project::snapshot::{self as snapshots, SNAPSHOTS_FILE, SnapshotError};

use crate::panel_write::sha256_hex;

use super::failure::{Failure, fail};

/// The `snapshots` message of `all` read from `text` (ADR 0041): the base,
/// the snapshots in the person's order (§9), the undo snapshot apart (§7).
pub(super) fn snapshot_list_json(all: &snapshots::Snapshots, text: Option<&str>) -> Value {
    let list: Vec<Value> = all
        .names()
        .iter()
        .filter_map(|name| {
            let values = all.snapshots.get(name)?;
            Some(json!({"name": name, "count": values.len(), "values": values}))
        })
        .collect();
    json!({
        "type": "snapshots",
        "file": SNAPSHOTS_FILE,
        "base": text.map(|t| sha256_hex(t.as_bytes())),
        "snapshots": list,
        "undo": all.undo.is_some(),
        "undoCount": all.undo.as_ref().map_or(0, |u| u.len()),
        "undoValues": all.undo,
        "transitionMs": all.transition_ms,
    })
}

/// A message's transition time under `key` (`transitionMs` on a restore or
/// undo, `ms` on `snapshot.transition`, and only that key): absent `None`;
/// else a whole number of milliseconds from 0 to 60000 (`bad-value`).
pub(super) fn transition_of(message: &Value, key: &str) -> Result<Option<u32>, Failure> {
    match message.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|ms| u32::try_from(ms).ok())
            .filter(|ms| *ms <= snapshots::MAX_TRANSITION_MS)
            .map(Some)
            .ok_or_else(|| {
                fail(
                    "bad-value",
                    format!(
                        "`{key}`: a transition is a whole number of milliseconds from 0 to {}",
                        snapshots::MAX_TRANSITION_MS
                    ),
                )
            }),
    }
}

/// A snapshot message's `name` (ADR 0041): trimmed, 1 to 64 characters.
pub(super) fn snapshot_name(message: &Value) -> Result<String, Failure> {
    let raw = message
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("bad-value", "a snapshot message needs `name` (text)"))?;
    snapshots::name(raw).map_err(snapshot_failure)
}

/// A snapshot write's check of the file's version (`base`).
pub(super) type BaseCheck = Box<dyn FnOnce(Option<&str>) -> Result<(), SnapshotError>>;

/// A snapshot message's `base`: absent takes the file as it is; null asks
/// that there be no `snapshots.toml`; else the SHA-256 it was listed with.
pub(super) fn base_check(message: &Value) -> Result<BaseCheck, Failure> {
    let expected: Option<Option<String>> = match message.get("base") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(base)) => Some(Some(base.clone())),
        Some(_) => return Err(fail("bad-value", "`base` must be a SHA-256 or null")),
    };
    Ok(Box::new(move |text: Option<&str>| {
        let Some(expected) = expected else {
            return Ok(());
        };
        let now = text.map(|t| sha256_hex(t.as_bytes()));
        if now == expected {
            Ok(())
        } else {
            Err(SnapshotError::Changed(format!(
                "{SNAPSHOTS_FILE} changed since it was listed (base); nothing was written"
            )))
        }
    }))
}

pub(super) fn snapshot_failure(error: SnapshotError) -> Failure {
    fail(error.code(), error.to_string())
}
