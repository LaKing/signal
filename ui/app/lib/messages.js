/**
 * The Parameters tab's message lines and the toolbar's muted notice
 * (docs/ui-controls.md "Signal.app's window"; `app.muted`,
 * docs/ui-protocol.md). Pure: tested by `node --test ui/`.
 */

/**
 * The lines shown, at most `max`: the pinned notes first (the muted
 * instances, which stay while the run lasts and must not be pushed out by
 * window events), then the newest events, then the other notes.
 * Each item is `{ text, className, pinned? }`.
 */
export function messageLines(events, notes, max) {
  const pinned = notes.filter((n) => n.pinned);
  const rest = notes.filter((n) => !n.pinned);
  return [...pinned, ...events, ...rest].slice(0, max);
}

/** The notes of an `app` message, the muted note pinned first. */
export function appNotes(app) {
  const notes = [];
  if (app.muted) notes.push({ text: app.muted, className: 'error', pinned: true });
  if (app.message) notes.push({ text: app.message, className: 'warn' });
  for (const d of app.project?.diagnostics ?? []) notes.push({ text: d, className: 'error' });
  for (const n of app.notes ?? []) notes.push({ text: n, className: '' });
  for (const p of app.setupProblems ?? []) notes.push({ text: p, className: 'warn' });
  return notes;
}

/**
 * The toolbar's notice when `app.muted` goes from nothing to a message (or
 * to another one), so that it is seen from every tab; `null` otherwise.
 */
export function mutedNotice(previous, muted) {
  if (!muted || muted === previous) return null;
  // The note names its first instance first; one that only says log records
  // were dropped (none listed) is shown as it is.
  if (!muted.startsWith('/')) return muted;
  const address = muted.split(' ')[0];
  return `Muted: ${address} is silent until the project plays again (see Parameters)`;
}
