// The host's unsaved parameters (docs/ui-protocol.md "Unsaved"): what
// `describe.unsaved` and the pushed `unsaved` message carry, as the controls,
// the panel and the app show it. No DOM.

/** The entry of `address` in an unsaved state, or null. */
export function unsavedOf(state, address) {
  if (!state || !address) return null;
  return (state.parameters ?? []).find((p) => p.address === address) ?? null;
}

/** "N unsaved parameters" and the last error; null when nothing is unsaved. */
export function unsavedSummary(state) {
  const count = state?.count ?? 0;
  if (!count) return null;
  return { text: `${count} unsaved parameter${count === 1 ? '' : 's'}`, error: state.lastError ?? null };
}

/** The summary as one line: "2 unsaved parameters — <last error>". */
export function unsavedLine(state) {
  const summary = unsavedSummary(state);
  if (!summary) return '';
  return summary.error ? `${summary.text} — ${summary.error}` : summary.text;
}

/** Writes an unsaved control's live value into project.toml again. */
export function retryMessage(address) {
  return { type: 'retry', address };
}

/** Sets an unsaved parameter's live value back to the file's. */
export function revertMessage(address) {
  return { type: 'revert', address };
}
