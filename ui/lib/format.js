// Values as text, with their unit (<signal-value>, control readouts).

function digitsFor(value) {
  const size = Math.abs(value);
  return size >= 100 ? 1 : size >= 10 ? 2 : 3;
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
