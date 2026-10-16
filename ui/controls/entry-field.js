// A readout that takes a typed value (docs/ui-controls.md "Value entry",
// M2.9-4): a double-click on it, or `open()` (Enter on its control), puts a
// text field over exactly the readout's box, out of the flow, so nothing
// moves. The states and the parsing are ui/lib/entry.js. Used by the
// controls, Signal.app's parameter list and the schematic's inspector.

import { EntryField, NOTE_MS } from '../lib/entry.js';

/** The field's look; the readout it covers needs `position: relative`. */
export const ENTRY_CSS = `
.entry {
  position: absolute;
  inset: 0;
  box-sizing: border-box;
  width: 100%;
  height: 100%;
  margin: 0;
  padding: 0 2px;
  border: 1px solid var(--signal-focus, #5aa9ff);
  border-radius: 3px;
  outline: none;
  background: var(--signal-panel, #1f232b);
  color: var(--signal-fg, #d6dbe4);
  font: inherit;
  text-align: inherit;
  user-select: text;
  -webkit-user-select: text;
}
.entry[aria-invalid="true"] { border-color: var(--signal-clip, #ff5252); }
.entry[readonly] { border-color: var(--signal-effective, #ffb74d); }
`;

/** Whether typed entry is off for `element`: it or an ancestor has `no-entry` (the panel editor's edit mode). */
export function entryDisabled(element) {
  return Boolean(element.closest?.('[no-entry]'));
}

/**
 * The value field of `readout`. `options`: `spec()` the parameter
 * (`{unit, range, taper, kind}`), `value()` the base value it opens with,
 * `apply(value)` sends a committed value, `note(text)` shows a reason in the
 * owner's message place (null clears it), `closed()` redraws the readout,
 * `focus()` gives the focus back to the owner, `enabled()` (default: always),
 * `next()` what Tab opens after a commit (the 2D pad's y after its x).
 */
export class ValueEntry {
  constructor(readout, options) {
    this.readout = readout;
    this.options = options;
    this.field = new EntryField();
    this.input = null;
    this.timer = null;
    readout.addEventListener('dblclick', (event) => {
      if (this.open()) event.stopPropagation();
    });
  }

  get isOpen() {
    return this.input !== null;
  }

  /** Whether a pointer event is over the readout (a captured pointer's events are the control's). */
  at(event) {
    const box = this.readout.getBoundingClientRect();
    return event.clientX >= box.left && event.clientX <= box.right && event.clientY >= box.top && event.clientY <= box.bottom;
  }

  open() {
    const enabled = this.options.enabled?.() ?? true;
    if (this.input || !enabled) return false;
    this.clearNote();
    const input = document.createElement('input');
    Object.assign(input, { type: 'text', className: 'entry', spellcheck: false, autocomplete: 'off' });
    input.setAttribute('aria-label', 'Type a value');
    input.value = this.field.open(this.options.value(), this.options.spec());
    // The field's pointer and keys are its own, not a drag or a key of the control.
    for (const type of ['pointerdown', 'pointermove', 'pointerup', 'click', 'dblclick', 'wheel', 'keyup']) {
      input.addEventListener(type, (event) => event.stopPropagation());
    }
    input.addEventListener('keydown', (event) => this.key(event));
    input.addEventListener('input', () => {
      this.field.edit(input.value);
      this.mark();
    });
    input.addEventListener('blur', () => {
      // A shown clamp stays its time; the timer closes it.
      if (this.input !== input || this.field.state === 'clamped') return;
      const result = this.field.blur();
      if (result.action === 'none' || result.action === 'cancel') this.close(false);
      else this.done(result, false);
    });
    this.input = input;
    this.mark();
    this.readout.append(input);
    input.focus({ preventScroll: true });
    input.select();
    return true;
  }

  key(event) {
    // Cmd/Ctrl shortcuts (Save, Open, …) reach the page: a changed text is
    // committed first, so Save writes what was typed.
    if (event.metaKey || event.ctrlKey) {
      if (this.field.text !== this.field.initial) this.done(this.field.commit(), false);
      return;
    }
    event.stopPropagation();
    if (event.repeat || event.isComposing) {
      if (event.key === 'Enter') event.preventDefault();
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      this.done(this.field.cancel(), true);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      this.done(this.field.commit(), true);
    } else if (event.key === 'Tab') {
      const result = this.field.commit();
      const next = result.action === 'commit' && !result.clamped && !event.shiftKey ? this.options.next : null;
      // An invalid text keeps the focus; a valid one lets Tab move it on,
      // and the blur that follows closes the field.
      if (result.action === 'invalid' || next) event.preventDefault();
      if (result.action === 'commit' && !result.clamped && !next) this.options.apply(result.value);
      else this.done(result, false);
      next?.();
    }
  }

  /** What a transition does: send, show the clamped value, mark invalid, or close. */
  done(result, refocus) {
    if (result.action === 'invalid') {
      this.mark();
      return;
    }
    if (result.action === 'commit') this.options.apply(result.value);
    if (result.action === 'commit' && result.clamped) {
      // The clamped value in the field and the reason in the message place,
      // for NOTE_MS, however it was committed (Enter or Escape on the field
      // closes it sooner; the reason stays its time).
      this.input.value = this.field.text;
      this.input.readOnly = true;
      this.mark();
      this.options.note(result.clamped);
      clearTimeout(this.timer);
      this.timer = setTimeout(() => {
        this.timer = null;
        this.options.note(null);
        if (this.field.state === 'clamped') this.close(this.readout.getRootNode().activeElement === this.input);
      }, NOTE_MS);
      return;
    }
    this.close(refocus);
  }

  /** The invalid mark and its tooltip, from the field's state. */
  mark() {
    if (!this.input) return;
    const invalid = this.field.state === 'invalid';
    this.input.setAttribute('aria-invalid', String(invalid));
    this.input.title = invalid ? this.field.error : this.field.state === 'clamped' ? '' : 'Enter or Tab: set; Escape: cancel';
  }

  clearNote() {
    if (this.timer === null) return;
    clearTimeout(this.timer);
    this.timer = null;
    this.options.note(null);
  }

  /** Closes the field (cancelling an edit); with `refocus`, the owner takes the focus back. */
  close(refocus = false) {
    const input = this.input;
    if (!input) return;
    if (this.field.isOpen) this.field.cancel();
    this.input = null;
    input.remove();
    this.options.closed?.();
    if (refocus) this.options.focus?.();
  }
}
