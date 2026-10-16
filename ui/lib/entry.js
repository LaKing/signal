// Typed value entry (docs/ui-controls.md "Value entry", M2.9-4): the text a
// field opens with, what a typed text means in a parameter's unit, and the
// field's states. Pure, tested with `node --test ui/`; the DOM is
// controls/entry-field.js.

import { formatValue } from './format.js';
import { effectiveTaper } from './taper.js';

/** How long a clamped value and its reason are shown. */
export const NOTE_MS = 2000;

const NUMBER = /^([+-]?)(\d+(?:[.,]\d*)?|[.,]\d+)(?:[eE]([+-]?\d+))?\s*(.*)$/;
const INFINITY = /^([+-]?)\s*(?:inf(?:inity)?|∞)\s*(.*)$/i;
const PREFIXES = { k: 1e3, m: 1e-3, M: 1e6 };

/**
 * Whether a parameter is a linear gain (dB typed as 10^(dB/20)): without a
 * unit, and of plan kind `gain` (describe's) or shown on a dB taper.
 */
function linearGain({ unit = null, range = {}, taper = 'linear', kind = null }) {
  return unit === null && (kind === 'gain' || effectiveTaper({ ...range, taper }) === 'db');
}

/**
 * The unit forms a parameter takes, lower case, each to a function of the
 * typed number: its own unit (Hz also as kHz, ms also as s, s also as ms),
 * dB for a linear gain, % for a range within 0..1.
 */
function unitForms(spec) {
  const { unit = null, range = {} } = spec;
  const forms = new Map();
  if (unit === 'Hz') {
    forms.set('hz', (n) => n);
    forms.set('khz', (n) => n * 1e3);
  } else if (unit === 'ms') {
    forms.set('ms', (n) => n);
    forms.set('s', (n) => n * 1e3);
  } else if (unit === 's') {
    forms.set('s', (n) => n);
    forms.set('ms', (n) => n / 1e3);
  } else if (unit) {
    forms.set(unit.toLowerCase(), (n) => n);
  }
  if (linearGain(spec)) forms.set('db', (n) => 10 ** (n / 20));
  if (unit !== '%' && range.min >= 0 && range.max <= 1) forms.set('%', (n) => n / 100);
  return forms;
}

function refuse(reason, error) {
  return { error, reason };
}

/** What a parameter is in, for a message: "a parameter in Hz", "a parameter without a unit". */
function inUnit(unit) {
  return unit ? `a parameter in ${unit}` : 'a parameter without a unit';
}

/**
 * The value `text` means for a parameter `{unit, range, taper, kind}`:
 * `{value}` (not clamped), or `{error, reason}` with `reason` one of
 * `empty`, `not-a-number`, `unit`, `not-finite`.
 *
 * A number: a sign, digits with a decimal comma or dot, an exponent
 * (`1e3`). Then, case-insensitively, one of the parameter's unit forms
 * (`unitForms`); else an SI prefix, exactly `k`, `m` (milli) or `M` (mega),
 * case-sensitive, with or without a unit form after it. A unit form is read
 * first, so `1ms` is a millisecond and `1M` a million, `1m` a thousandth.
 * `-inf`, `-∞` (with or without `dB`) is the minimum of a dB taper; any
 * other infinity is refused.
 */
export function parseValue(text, spec = {}) {
  const { unit = null, range = {}, taper = 'linear' } = spec;
  // The minus sign (U+2212) Signal's own formatter prints reads as `-`.
  const trimmed = String(text ?? '').trim().replace(/\u2212/g, '-');
  if (trimmed === '') return refuse('empty', 'Type a value');
  const forms = unitForms(spec);
  const infinite = INFINITY.exec(trimmed);
  if (infinite) {
    const suffix = infinite[2].toLowerCase();
    const db = suffix === 'db' || (suffix === '' && (unit === 'dB' || linearGain(spec)));
    // −∞ dB is the minimum: on a dB taper, of a parameter in dB, and of a
    // linear gain that reaches 0.
    const bottom = effectiveTaper({ ...range, taper }) === 'db' || unit === 'dB' || (linearGain(spec) && range.min === 0);
    if (infinite[1] === '-' && db && bottom && Number.isFinite(range.min)) return { value: range.min };
    return refuse('not-finite', `${trimmed} is not a finite value`);
  }
  const match = NUMBER.exec(trimmed);
  if (!match) return refuse('not-a-number', `"${trimmed}" is not a number`);
  const [, sign, digits, exponent, rest] = match;
  const number = Number(`${sign}${digits.replace(',', '.')}${exponent === undefined ? '' : `e${exponent}`}`);
  if (Number.isNaN(number)) return refuse('not-a-number', `"${trimmed}" is not a number`);
  let value;
  const suffix = rest.trim();
  if (suffix === '') {
    value = number;
  } else if (forms.has(suffix.toLowerCase())) {
    value = forms.get(suffix.toLowerCase())(number);
  } else if (suffix[0] in PREFIXES) {
    const after = suffix.slice(1).trim();
    const form = after === '' ? (n) => n : forms.get(after.toLowerCase());
    // A prefix before % or dB means nothing.
    if (!form || after === '%' || after.toLowerCase() === 'db') {
      return refuse('unit', `"${suffix}" does not fit ${inUnit(unit)}`);
    }
    value = form(number * PREFIXES[suffix[0]]);
  } else {
    return refuse('unit', `"${suffix}" does not fit ${inUnit(unit)}`);
  }
  if (!Number.isFinite(value)) return refuse('not-finite', `${trimmed} is not a finite value`);
  return { value };
}

/**
 * The text a field opens with: the value in full precision (the shortest
 * text that reads back to the same number) and the parameter's unit, so
 * `parseValue` gives the value back.
 */
export function entryText(value, unit = null) {
  if (!Number.isFinite(value)) return '';
  return unit ? `${String(value)} ${unit}` : String(value);
}

/**
 * `value` within `range`: `{value, clamped}`, `clamped` the reason when it
 * was moved to the minimum or the maximum, else null.
 */
export function clampEntry(value, { min, max, unit = null }) {
  if (value > max) return { value: max, clamped: `Clamped to the maximum, ${formatValue(max, unit)}` };
  if (value < min) return { value: min, clamped: `Clamped to the minimum, ${formatValue(min, unit)}` };
  return { value, clamped: null };
}

/**
 * A value field's states: `closed`; `open` (being edited); `invalid` (a
 * commit refused its text: it stays open, nothing is sent); `clamped` (a
 * value was committed at the range's end and is shown, with the reason,
 * for `NOTE_MS`). Each transition returns what the field does:
 * `{action: 'commit', value, clamped}`, `{action: 'invalid', error,
 * reason}`, `{action: 'cancel'}` or `{action: 'none'}`.
 */
export class EntryField {
  constructor() {
    this.state = 'closed';
    this.text = '';
    this.initial = '';
    this.error = null;
    this.spec = null;
  }

  get isOpen() {
    return this.state !== 'closed';
  }

  /** Opens on `value` for a parameter `spec` ({unit, range, taper, kind}); returns the text. */
  open(value, spec) {
    this.spec = spec;
    this.state = 'open';
    this.error = null;
    this.initial = this.text = entryText(value, spec.unit ?? null);
    return this.text;
  }

  /** The text was changed. */
  edit(text) {
    if (this.state !== 'open' && this.state !== 'invalid') return { action: 'none' };
    this.text = text;
    this.state = 'open';
    this.error = null;
    return { action: 'none' };
  }

  /** Enter or Tab: an unchanged text cancels (nothing is sent or written). */
  commit() {
    if (this.state === 'clamped') return this.cancel();
    if (!this.isOpen) return { action: 'none' };
    if (this.text === this.initial) return this.cancel();
    const parsed = parseValue(this.text, this.spec);
    if ('error' in parsed) {
      this.state = 'invalid';
      this.error = parsed.error;
      return { action: 'invalid', error: parsed.error, reason: parsed.reason };
    }
    const { value, clamped } = clampEntry(parsed.value, { ...this.spec.range, unit: this.spec.unit ?? null });
    if (clamped) {
      this.state = 'clamped';
      this.text = entryText(value, this.spec.unit ?? null);
    } else {
      this.state = 'closed';
    }
    return { action: 'commit', value, clamped };
  }

  /** Escape, or the clamped value's time is up. */
  cancel() {
    if (!this.isOpen) return { action: 'none' };
    this.state = 'closed';
    this.error = null;
    return { action: 'cancel' };
  }

  /**
   * The field lost the focus: a changed text commits, an unchanged one
   * cancels; a shown clamp stays its time (`cancel` ends it).
   */
  blur() {
    if (this.state === 'clamped') return { action: 'none' };
    // An invalid text is let go: the field never stays open unfocused.
    if (this.state === 'invalid') return this.cancel();
    return this.commit();
  }
}
