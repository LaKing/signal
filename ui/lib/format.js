// Values as text, with their unit (<signal-value>, control readouts).

function digitsFor(value) {
  const size = Math.abs(value);
  return size >= 100 ? 1 : size >= 10 ? 2 : 3;
}

/** The characters of a fixed-width number (docs/ui-controls.md "Stable layout"). */
export const NUMBER_WIDTH = 8;

// The longest unit each one may be shown in (Hz as kHz, s as ms, ms as s).
const UNIT_WIDTH = Object.freeze({ Hz: 3, s: 2, ms: 2 });

/** The characters `formatFixed` always gives for `unit`. */
export function fixedWidth(unit = null) {
  return unit ? NUMBER_WIDTH + 1 + Math.max(String(unit).length, UNIT_WIDTH[unit] ?? 0) : NUMBER_WIDTH;
}

/**
 * `formatValue` at a fixed width, `fixedWidth(unit)` characters, whatever
 * the value: the number right-aligned (in exponent form when it would not
 * fit), the unit left-aligned. A readout with it, in tabular figures and a
 * box of that width, never moves what is beside it.
 */
export function formatFixed(value, unit = null, options = {}) {
  const text = formatValue(value, unit, options);
  const space = text.indexOf(' ');
  let number = space < 0 ? text : text.slice(0, space);
  const shownUnit = space < 0 ? '' : text.slice(space + 1);
  if (number.length > NUMBER_WIDTH) {
    const v = Number(number);
    number = v.toExponential(1).length <= NUMBER_WIDTH ? v.toExponential(1) : v.toExponential(0);
  }
  const padded = number.padStart(NUMBER_WIDTH);
  return unit ? `${padded} ${shownUnit.padEnd(fixedWidth(unit) - NUMBER_WIDTH - 1)}` : padded;
}

/**
 * `value` in `unit`: Hz above 1000 as kHz, seconds below 1 as ms, ms from
 * 1000 as s, −∞ for silence, — when unknown. `digits` fixes the decimals.
 */
export function formatValue(value, unit = null, { digits = null } = {}) {
  if (Number.isNaN(value) || value === null || value === undefined) return '—';
  if (value === -Infinity) return unit ? `−∞ ${unit}` : '−∞';
  if (value === Infinity) return unit ? `∞ ${unit}` : '∞';
  const fixed = (v, d) => v.toFixed(digits ?? d ?? digitsFor(v));
  switch (unit) {
    case 'Hz':
      return Math.abs(value) >= 1000 ? `${fixed(value / 1000, 2)} kHz` : `${fixed(value, 1)} Hz`;
    case 'dB':
      return `${fixed(value, 1)} dB`;
    case 's':
      return Math.abs(value) < 1 ? `${fixed(value * 1000, 1)} ms` : `${fixed(value, 2)} s`;
    case 'ms':
      return Math.abs(value) >= 1000 ? `${fixed(value / 1000, 2)} s` : `${fixed(value, 1)} ms`;
    default:
      return unit ? `${fixed(value)} ${unit}` : fixed(value);
  }
}
