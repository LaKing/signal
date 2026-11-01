//! A refused message: its code, why, and what else its error carries.

use serde_json::Value;

/// A refused message: its code and why, and what else the error carries
/// (`panel.write`'s `conflict`: the current text and base).
pub(super) struct Failure {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) detail: Option<Value>,
}

pub(super) fn fail(code: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
        detail: None,
    }
}

/// [`fail`], with `detail`'s fields in the error.
pub(super) fn fail_with(code: &'static str, message: impl Into<String>, detail: Value) -> Failure {
    Failure {
        detail: Some(detail),
        ..fail(code, message)
    }
}
