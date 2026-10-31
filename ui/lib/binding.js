// The binding of a control to one parameter (docs/ui-controls.md): its range,
// base and effective values, the live edits and the commits, and whether
// the edit is unsaved (docs/ui-protocol.md "Unsaved"). No DOM, so
// `node --test ui/` can load it; controls/base.js builds on it.

import { findParameter } from '../signal-ui.js';
import { formatValue } from './format.js';
import { resetValue } from './gesture.js';
import { toPosition, toValue } from './taper.js';
import { unsavedOf } from './unsaved.js';

/** Keyboard and wheel edits are committed once this long after the last one. */
export const COMMIT_DELAY_MS = 500;

/** The ramp a drag sends with each move. */
export const RAMP_MS = 5;

/**
 * A live edit whose refusal shows in the next parameter frame, not as a
 * console error. Never for a commit: a refused commit is shown (`Binding`).
 */
export function quietly(promise) {
  promise?.catch?.(() => {});
}

/** The open project's generation a connection last heard of (ADR 0049), or null. */
export function generationOf(connection) {
  const generation = connection?.generation ?? connection?.description?.generation;
  return Number.isSafeInteger(generation) ? generation : null;
}

/** A numeric attribute, or undefined when absent or not a number. */
export function numberAttribute(element, name) {
  const text = element.getAttribute(name);
  if (text === null || text.trim() === '') return undefined;
  const value = Number(text);
  return Number.isFinite(value) ? value : undefined;
}

/**
 * One parameter a control edits: its range (from `describe`, with the
 * element's `min`, `max`, `default`, `unit`, `taper` and `floor` attributes
 * over it), its base and effective values (from a `parameters`
 * subscription), and the edits: a ramp per drag move and a commit on
 * release, or a live `set` per key or wheel step and one commit
 * `COMMIT_DELAY_MS` after the last.
 *
 * `unsaved` is `{error}` while the host lists the parameter with an error,
 * or once a commit, retry or revert of this binding was refused, until one
 * is confirmed or the host no longer lists it; null otherwise. `onChange` is
 * called when it changes.
 */
export class Binding {
  constructor(element, address) {
    this.element = element;
    this.address = address;
    this.described = null;
    this.base = NaN;
    this.effective = NaN;
    this.timer = null;
    this.pending = false;
    this.unsaved = null;
    this.onChange = null;
    /**
     * The opening of the project the edit in progress began under (ADR
     * 0049): its commit names it, so a commit flushed after another project
     * opened (a debounced key edit, a release, a flush on unload) is refused
     * by the host instead of written into the new one.
     */
    this.editGeneration = undefined;
  }

  get range() {
    const e = this.element;
    const d = this.described ?? {};
    return {
      min: numberAttribute(e, 'min') ?? d.min ?? 0,
      max: numberAttribute(e, 'max') ?? d.max ?? 1,
      default: numberAttribute(e, 'default') ?? d.default,
      unit: e.getAttribute('unit') ?? d.unit ?? null,
      taper: e.getAttribute('taper') ?? 'linear',
      floorDb: numberAttribute(e, 'floor'),
    };
  }

  /** The value shown: the base, else the default. */
  get value() {
    return Number.isFinite(this.base) ? this.base : resetValue(this.range);
  }

  get position() {
    return toPosition(this.value, this.range);
  }

  get effectivePosition() {
    return toPosition(this.effective, this.range);
  }

  /** Whether the effective value differs visibly from the base (modulated). */
  get modulated() {
    return Number.isFinite(this.effective) && Math.abs(this.effectivePosition - this.position) > 0.002;
  }

  describe(description) {
    this.described = findParameter(description, this.address);
    if (this.described && !this.pending && !this.element.dragging) {
      const base = this.described.base ?? this.described.default;
      if (Number.isFinite(base)) this.base = base;
    }
    if (description && 'unsaved' in description) this.fromHost(description.unsaved);
  }

  /** A parameter frame's `{base, effective}` (NaN until the runner reported it). */
  frame({ base, effective }) {
    if (!this.pending && !this.element.dragging && Number.isFinite(base)) this.base = base;
    this.effective = effective;
  }

  /**
   * The host's unsaved state (`describe.unsaved` or the pushed `unsaved`):
   * its error for this parameter is shown; a parameter it no longer lists is
   * saved. An entry without an error (an edit not committed yet) keeps a
   * refusal this binding saw (a commit lost with its connection).
   */
  fromHost(state) {
    const entry = unsavedOf(state, this.address);
    const next = entry?.error ? { error: entry.error } : entry ? this.unsaved : null;
    this.setUnsaved(next);
  }

  setUnsaved(unsaved) {
    if (unsaved?.error === this.unsaved?.error) return;
    this.unsaved = unsaved;
    this.onChange?.();
  }

  /**
   * Moves to `position`: `how` is `ramp` (drag), `nudge` (key, wheel),
   * `commit`, or `local` (shown only; the caller sends).
   */
  moveTo(position, how) {
    const value = toValue(position, this.range);
    this.setValue(value, how);
  }

  setValue(value, how) {
    this.base = value;
    const connection = this.element.connection;
    if (how === 'local') this.cancel();
    if (!connection || !this.address || how === 'local') return;
    if (how === 'ramp') {
      this.begin(connection);
      quietly(connection.ramp(this.address, value, { ms: RAMP_MS }));
    } else if (how === 'nudge') {
      this.begin(connection);
      quietly(connection.set(this.address, value));
      this.pending = true;
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.flush(), COMMIT_DELAY_MS);
    } else {
      const generation = this.editGeneration;
      this.cancel();
      this.commit(value, generation);
    }
  }

  /** An edit begins: the project's opening it is meant for is kept. */
  begin(connection) {
    if (this.editGeneration === undefined) this.editGeneration = generationOf(connection);
  }

  /** A typed value (docs/ui-controls.md "Value entry"): sent as a drag's release is, a short ramp, then committed. */
  enter(value) {
    this.setValue(value, 'ramp');
    this.setValue(value, 'commit');
  }

  /** Commits now what a debounced edit left pending. */
  flush() {
    if (!this.pending) return;
    const generation = this.editGeneration;
    this.cancel();
    this.commit(this.base, generation);
  }

  cancel() {
    clearTimeout(this.timer);
    this.timer = null;
    this.pending = false;
    this.editGeneration = undefined;
  }

  reset() {
    this.setValue(resetValue(this.range), 'commit');
  }

  /**
   * Writes `value` into project.toml; a refusal is kept and shown.
   * `generation`: the opening the edit began under (now's by default).
   */
  commit(value, generation = undefined) {
    const connection = this.element.connection;
    if (!connection || !this.address) return Promise.resolve(null);
    const meant = generation === undefined ? generationOf(connection) : generation;
    return this.settle(connection.commit(this.address, value, { generation: meant }));
  }

  /** Writes the host's live value again (`retry`). */
  retry() {
    const connection = this.element.connection;
    if (!connection || !this.address) return Promise.resolve(null);
    return this.settle(connection.retry(this.address, { generation: generationOf(connection) }));
  }

  /** Sets the live value back to the file's (`revert`). */
  revert() {
    const connection = this.element.connection;
    if (!connection || !this.address) return Promise.resolve(null);
    return this.settle(connection.revert(this.address, { generation: generationOf(connection) }));
  }

  /**
   * A write's (or revert's) reply: confirmed, nothing is unsaved here; refused,
   * the error stays until one is confirmed (`not-unsaved`: the host has
   * nothing to save, so nothing is). Resolves with the reply, or null.
   */
  settle(request) {
    return request.then(
      (reply) => {
        this.setUnsaved(null);
        return reply;
      },
      (error) => {
        this.setUnsaved(error.code === 'not-unsaved' ? null : { error: error.message ?? String(error) });
        return null;
      },
    );
  }

  ariaText() {
    const unit = this.range.unit;
    const text = formatValue(this.value, unit);
    return this.modulated ? `${text}, modulated to ${formatValue(this.effective, unit)}` : text;
  }
}
