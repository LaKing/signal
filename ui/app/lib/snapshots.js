// Snapshots (ADR 0041, docs/ui-protocol.md "Snapshots"): every top-level
// parameter's value stored under a name in the project's snapshots.toml,
// saved, restored and deleted from the Parameters tab's Snapshots card. The
// server checks everything again; this is what the card shows and sends.
// Plain ES module, no DOM.

/** The longest name, in characters, after trimming (the server's rule). */
export const MAX_NAME = 64;

/**
 * A typed name as the server takes it: trimmed, 1 to 64 characters (code
 * points), no control character. `{ name }`, or `{ error }` saying why not.
 */
export function snapshotName(raw) {
  const name = String(raw ?? '').trim();
  if (!name) return { error: 'Type a name for the snapshot.' };
  const length = [...name].length;
  if (length > MAX_NAME) return { error: `A name is at most ${MAX_NAME} characters (this one has ${length}).` };
  if (/[\u0000-\u001f\u007f-\u009f]/u.test(name)) return { error: 'A name cannot hold a tab or a line break.' };
  if (/[\p{Cf}\u2028\u2029]/u.test(String(raw ?? ''))) return { error: 'A name cannot hold an invisible character (a zero-width space, a byte-order mark, a direction mark) or a line separator.' };
  if (/^[\p{White_Space}\p{Cf}\u034f\u115f\u1160\u17b4\u17b5\u2800\u3164\uffa0]+$/u.test(name)) return { error: 'A name must show something (it is all blank characters).' };
  // Not normalised: a composed "é" and "e" + combining accent are two names, as on the server.
  return { name };
}

/**
 * What the card shows instead of the list: why the list could not be read
 * (a `snapshots.toml` that does not read, a link, no project), or, before
 * any answer, that a project must be open.
 */
export function listProblem(error) {
  if (!error) return 'Snapshots appear once a project is open.';
  if (error.code === 'no-project') return 'Snapshots appear once a project is open.';
  return `Snapshots cannot be read: ${error.message ?? String(error)}`;
}

/** The card's rows from a `snapshots` message: `{ name, count }` in the server's (byte) order. */
export function entries(list) {
  return (list?.snapshots ?? []).map((s) => ({ name: s.name, count: s.count ?? Object.keys(s.values ?? {}).length }));
}

/** Whether `list` has a snapshot named `name` (so Save asks before it replaces it). */
export function exists(list, name) {
  return entries(list).some((e) => e.name === name);
}

/** `snapshot.list`. */
export function listMessage() {
  return { type: 'snapshot.list' };
}

/**
 * `snapshot.save` of `name`: `overwrite` once the person said Replace, and
 * the `base` the list was read with, so another writer's change meanwhile
 * is a `conflict` rather than a replaced snapshot nobody saw.
 */
export function saveMessage(name, { overwrite = false, base } = {}) {
  return { type: 'snapshot.save', name, ...(overwrite ? { overwrite: true } : {}), ...(base === undefined ? {} : { base }) };
}

/** `snapshot.restore` of `name`, as listed (`base`). */
export function restoreMessage(name, base) {
  return { type: 'snapshot.restore', name, ...(base === undefined ? {} : { base }) };
}

/** `snapshot.undo` (ADR 0041 §7): the undo snapshot restored, as listed (`base`). */
export function undoMessage(base) {
  return { type: 'snapshot.undo', ...(base === undefined ? {} : { base }) };
}

/** Whether the list has an undo snapshot, so the card offers Undo restore. */
export function hasUndo(list) {
  return Boolean(list?.undo);
}

/** `snapshot.exclude` (ADR 0041 §8): a control left out of snapshots, or put back in. */
export function excludeMessage(address, exclude) {
  return { type: 'snapshot.exclude', address, exclude: Boolean(exclude) };
}

/** The card's line after an undo: what came back (a second undo redoes). */
export function undoneText(reply) {
  const { text, warn } = restoredText({ ...reply, name: 'the state before the last restore' });
  return { text: `${text.replace(/^Restored “the state before the last restore”/, 'Undid the last restore')} Undo again to redo it.`, warn };
}

/** The longest snapshot transition, in ms (the server's rule, ADR 0041 §10). */
export const MAX_TRANSITION_MS = 60000;

/**
 * The Transition field's text as the server takes it: a whole number of
 * milliseconds from 0 to 60000 (empty: 0). `{ ms }`, or `{ error }`.
 */
export function transitionValue(raw) {
  const text = String(raw ?? '').trim();
  if (!text) return { ms: 0 };
  if (!/^\d+$/.test(text)) return { error: 'The transition is a whole number of milliseconds.' };
  const ms = Number(text);
  if (ms > MAX_TRANSITION_MS) return { error: `The transition is at most ${MAX_TRANSITION_MS} ms.` };
  return { ms };
}

/** `snapshot.transition` (ADR 0041 §10): the project's snapshot transition time, as listed (`base`). */
export function transitionMessage(ms, base) {
  return { type: 'snapshot.transition', ms, ...(base === undefined ? {} : { base }) };
}

/** The list's transition time (0 from an older server). */
export function transitionOf(list) {
  return Number.isInteger(list?.transitionMs) ? list.transitionMs : 0;
}

/** The card's line after the transition time was set. */
export function transitionText(ms) {
  return ms === 0 ? 'Restores switch at once (no transition).' : `Restores move over ${ms} ms (stepped controls switch at the start).`;
}

/** `snapshot.move` (ADR 0041 §9): `name` to place `to` (0-based), as listed (`base`). */
export function moveMessage(name, to, base) {
  return { type: 'snapshot.move', name, to, ...(base === undefined ? {} : { base }) };
}

/**
 * Where a keyboard command moves the row at `index` of `count` rows:
 * `snapshot_list.move_up` one place up, `move_down` one down; null at an
 * edge or for another command (nothing to send).
 */
export function keyMove(command, index, count) {
  if (command === 'snapshot_list.move_up' && index > 0) return index - 1;
  if (command === 'snapshot_list.move_down' && index < count - 1) return index + 1;
  return null;
}

/**
 * Whether a drop over a row lands after it: the pointer in its lower half
 * (`offsetY` from the row's top, `height` the row's).
 */
export function dropsAfter(offsetY, height) {
  return offsetY >= height / 2;
}

/**
 * Where a pointer at `clientY` drops among rows whose boxes (their
 * `getBoundingClientRect()`, `{ top, bottom }`, in order) are `rects`:
 * `{ over, after }`, the row it is over (the first above all, the last
 * below all) and whether in its lower half; null without rows.
 */
export function dropTarget(rects, clientY) {
  if (!rects.length) return null;
  let over = rects.findIndex((r) => clientY < r.bottom);
  if (over < 0) over = rects.length - 1;
  const r = rects[over];
  return { over, after: clientY >= r.top + (r.bottom - r.top) / 2 };
}

/**
 * How far to scroll a view whose box is `top`..`bottom` while dragging at
 * `clientY`: within `edge` px of an edge, up to `most` px a move toward it
 * (negative: up); 0 elsewhere.
 */
export function autoScroll(clientY, top, bottom, { edge = 40, most = 20 } = {}) {
  if (clientY < top + edge) return -Math.ceil(most * Math.min(1, (top + edge - clientY) / edge));
  if (clientY > bottom - edge) return Math.ceil(most * Math.min(1, (clientY - (bottom - edge)) / edge));
  return 0;
}

/**
 * A drag of a row started on its handle (ADR 0041 §9): what it moves (the
 * name and the list's base when it started, never a place that a push
 * could change), and the place it would take now. `dragStart` begins it,
 * `dragOver` follows the pointer, `dragDrop` gives the `snapshot.move` to
 * send (null: nothing moves, or the list changed underneath).
 */
export function dragStart(list, name) {
  const from = entries(list).findIndex((e) => e.name === name);
  return from < 0 ? null : { name, base: list?.base ?? null, from, target: null };
}

export function dragOver(drag, rects, clientY) {
  return drag ? { ...drag, target: dropTarget(rects, clientY) } : null;
}

export function dragDrop(drag, list) {
  if (!drag?.target) return null;
  // The list changed underneath (a push from another client): nothing moves.
  if ((list?.base ?? null) !== drag.base) return null;
  if (entries(list)[drag.from]?.name !== drag.name) return null;
  const to = dropIndex(drag.from, drag.target.over, drag.target.after);
  return to === null ? null : moveMessage(drag.name, to, drag.base ?? undefined);
}

/**
 * The place a row dragged from `from` takes when dropped before (or
 * `after`) the row at `over`, as `snapshot.move` counts it (the list
 * without the dragged row); null when it stays where it is.
 */
export function dropIndex(from, over, after) {
  const at = over + (after ? 1 : 0);
  const to = from < at ? at - 1 : at;
  return to === from ? null : to;
}

/** `snapshot.delete` of `name`, as listed (`base`). */
export function deleteMessage(name, base) {
  return { type: 'snapshot.delete', name, ...(base === undefined ? {} : { base }) };
}

/** "N values". */
export function countText(count) {
  return count === 1 ? '1 value' : `${count} values`;
}

/** The confirmations: their question and buttons, the safe one (Cancel) first and focused. */
export function overwritePrompt(name) {
  return { text: `Replace the snapshot “${name}” with the values playing now?`, buttons: [{ id: 'cancel', label: 'Cancel' }, { id: 'confirm', label: 'Replace' }] };
}

export function deletePrompt(name) {
  return { text: `Delete the snapshot “${name}”? This cannot be undone.`, buttons: [{ id: 'cancel', label: 'Cancel' }, { id: 'confirm', label: 'Delete' }] };
}

/** The card's line after a save. */
export function savedText(reply) {
  return `${reply.replaced ? 'Replaced' : 'Saved'} “${reply.name}” (${countText(reply.count ?? 0)}).`;
}

/**
 * The card's line after a restore, and whether it is a warning: what was
 * restored, and what was skipped (gone instances or parameters) or clamped
 * (out of range), named.
 */
export function restoredText(reply) {
  const skipped = reply.skipped ?? [];
  const clamped = reply.clamped ?? [];
  const written = (reply.written ?? []).length;
  const parts = [`Restored “${reply.name}”: ${countText(reply.restored ?? 0)}, ${written} written to ${reply.file ?? 'project.toml'}`];
  const gone = skipped.filter((s) => s.reason !== 'excluded');
  const excluded = skipped.filter((s) => s.reason === 'excluded');
  if (gone.length) parts.push(`skipped ${gone.map((s) => s.address).join(', ')} (no longer in the project)`);
  if (excluded.length) parts.push(`left ${excluded.map((s) => s.address).join(', ')} alone (not in snapshots)`);
  if (clamped.length) parts.push(`clamped ${clamped.map((c) => `${c.address} to ${c.to}`).join(', ')}`);
  if (reply.sent === false) parts.push('not every value reached the engine; it stays unsaved until it does');
  return { text: `${parts.join('; ')}.`, warn: Boolean(gone.length || clamped.length || reply.sent === false) };
}

/** The card's line after a delete. */
export function deletedText(reply) {
  return `Deleted “${reply.name}”.`;
}

/** A refusal as the card says it: the server's message, a conflict with what to do. */
export function refusalText(error) {
  if (error?.code === 'conflict') return `${error.message} The list was read again; try once more.`;
  return error?.message ?? String(error ?? '');
}
