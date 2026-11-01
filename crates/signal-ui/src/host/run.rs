//! The run: its build, install, poll and stop, the instances the fault guard
//! mutes, and the device wait.

use std::collections::BTreeMap;

use signal_core::{Frame, PortAddress};
use signal_engine::{
    ControlSink, FaultReason, HangLine, LogLine, ParameterValue, Report, SentCommand,
};
use signal_graph::ExecutionPlan;

use super::Run;

/// The values, reports and mutes of one poll, beside the session recorder.
pub(super) struct Observed<'a> {
    pub(super) values: &'a mut BTreeMap<PortAddress, ParameterValue>,
    pub(super) reports: &'a mut Vec<Report>,
    pub(super) muted: &'a mut Vec<Muted>,
}

/// An instance the fault guard muted (`docs/semantics.md` §7, §14): it is
/// never called again in this run, so its outputs stay 0 whatever its
/// parameters are set to.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Muted {
    frame: Frame,
    pub(super) address: String,
    reason: FaultReason,
    /// The instance's module type when it was muted (from the plan then):
    /// a swap that removes the instance or gives the address another type
    /// brings a new, unfaulted one (`semantics.md` §14), and the entry goes.
    pub(super) module: Option<String>,
}

/// How many other muted instances the note names before "and N more".
const MUTED_LISTED: usize = 8;

impl Muted {
    /// What the operator is told: which instance, why, and how it comes back.
    fn message(&self) -> String {
        let (why, back) = match self.reason {
            FaultReason::NonFinite => (
                "an output or event value that is not finite",
                "with its values back in range, play the project again",
            ),
            FaultReason::Magnitude => (
                "an output or event value beyond ±10⁶",
                "with its values back in range, play the project again",
            ),
            FaultReason::EventOverflow => (
                "more than 16 events on an output in one frame",
                "with fewer events reaching it, play the project again",
            ),
            FaultReason::Panic => (
                "its call panicked",
                "its module broke the module contract; play the project again",
            ),
            FaultReason::LateResult => ("a late result", "play the project again"),
        };
        format!(
            "{} was muted at frame {} ({why}): it stays silent for the rest of this run, \
             whatever its parameters are set to; {back} (Project ▸ Restart from Files, or the \
             power off and on) to hear it",
            self.address, self.frame.0
        )
    }
}

/// Signal.app's note for the instances muted in a run: the first one's
/// message, the addresses of up to [`MUTED_LISTED`] others and how many
/// more, and, when the run dropped log records, that the list may be short
/// (a fault record may be among them). `None` when there is nothing to say.
pub(super) fn muted_message(muted: &[Muted], log_lost: bool) -> Option<String> {
    const LOST: &str =
        "log records were dropped, so an instance the fault guard muted may be missing here";
    let Some((first, rest)) = muted.split_first() else {
        return log_lost.then(|| format!("Some {LOST}"));
    };
    let mut text = first.message();
    if !rest.is_empty() {
        let named: Vec<&str> = rest
            .iter()
            .take(MUTED_LISTED)
            .map(|m| m.address.as_str())
            .collect();
        let more = rest.len().saturating_sub(MUTED_LISTED);
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        text = format!("{text} (also muted: {}{tail})", named.join(", "));
    }
    if log_lost {
        text = format!("{text}; some {LOST}");
    }
    Some(text)
}

/// Takes the mutes `pending` (from a poll, or a reload's poll) into `run`,
/// each said once in `messages`, with the instance's module type from
/// `plan`; the note is made again when anything was taken.
pub(super) fn take_muted(
    pending: &mut Vec<Muted>,
    run: &mut Run,
    plan: Option<&ExecutionPlan>,
    messages: &mut Vec<String>,
) {
    if pending.is_empty() {
        return;
    }
    for mut muted in pending.drain(..) {
        muted.module = plan.and_then(|plan| module_at(plan, &muted.address));
        messages.push(muted.message());
        run.muted.push(muted);
    }
    run.muted_note = muted_message(&run.muted, run.log_lost);
}

/// The module type of the instance at `address` in `plan`.
pub(super) fn module_at(plan: &ExecutionPlan, address: &str) -> Option<String> {
    plan.instances
        .iter()
        .find(|i| i.address.as_str() == address)
        .map(|i| i.descriptor.type_name.to_owned())
}

impl ControlSink for Observed<'_> {
    fn report(&mut self, report: &Report, _command: &SentCommand) {
        self.reports.push(*report);
    }
    /// A fault that mutes its instance for the run: every reason but
    /// `late_result`, which only silences one block of its own (§7).
    fn log(&mut self, line: LogLine<'_>) {
        if let LogLine::Fault {
            frame,
            address,
            reason,
            ..
        } = line
            && reason != FaultReason::LateResult
        {
            self.muted.push(Muted {
                frame,
                address: address.to_string(),
                reason,
                module: None,
            });
        }
    }
    fn parameter(&mut self, port: &PortAddress, value: ParameterValue) {
        match self.values.get_mut(port) {
            Some(known) => *known = value,
            None => {
                self.values.insert(port.clone(), value);
            }
        }
    }
    /// A hung callback (M2.7-9): a diagnosis on stderr, not recorded.
    fn hang(&mut self, line: HangLine<'_>) {
        let running: Vec<String> = line.running.iter().map(ToString::to_string).collect();
        eprintln!(
            "signal: hang: callback {} has not returned after {:.1} ms; still running: {}",
            line.callback,
            line.waited.as_secs_f64() * 1e3,
            running.join(", ")
        );
    }
}

/// Whether an output as [`output_state`] gives it can play: it is there, at
/// any rate (ADR 0036: a run plays at the rate the output runs at).
pub(super) fn ready_at(state: Option<&(String, Option<u32>)>) -> bool {
    state.is_some()
}

/// Stops a run on this thread; its Runner and streams are dropped here.
pub(super) fn stop(mut run: Run) {
    let _ = run.output.stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn muted(address: &str, reason: FaultReason) -> Muted {
        Muted {
            frame: Frame(7),
            address: address.to_owned(),
            reason,
            module: None,
        }
    }

    /// The note: the recovery said by reason; dropped log records said,
    /// alone too; nothing to say without either.
    #[test]
    fn the_muted_note_says_how_and_whether_the_list_is_whole() {
        assert_eq!(muted_message(&[], false), None);
        let lost = muted_message(&[], true).unwrap();
        assert!(lost.starts_with("Some log records were dropped"), "{lost}");
        let magnitude = muted_message(&[muted("/a", FaultReason::Magnitude)], false).unwrap();
        assert!(
            magnitude.contains("with its values back in range"),
            "{magnitude}"
        );
        let panic = muted("/b", FaultReason::Panic).message();
        assert!(
            panic.contains("its call panicked") && panic.contains("broke the module contract"),
            "{panic}"
        );
        assert!(!panic.contains("values back in range"), "{panic}");
        let overflow = muted("/c", FaultReason::EventOverflow).message();
        assert!(
            overflow.contains("with fewer events reaching it"),
            "{overflow}"
        );
        let both = muted_message(&[muted("/a", FaultReason::NonFinite)], true).unwrap();
        assert!(
            both.starts_with("/a was muted at frame 7")
                && both.ends_with("an instance the fault guard muted may be missing here"),
            "{both}"
        );
    }
}
