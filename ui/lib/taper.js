// Value ↔ position for the controls (docs/ui-controls.md "Tapers"). A
// position is 0..1 along a control's travel; a value is in the parameter's
// own unit, between `min` and `max`. Pure functions, tested with
// `node --test ui/`.

/** The dB taper's lowest dB above position 0 when `min` is 0. */
export const DEFAULT_FLOOR_DB = -60;

export function clamp(x, lo, hi) {
  return Math.min(hi, Math.max(lo, x));
}

/**
 * The taper a range really uses: `log` needs `0 < min < max`, `db` needs
 * `0 <= min < max`; otherwise (or for an unknown name) `linear`.
 */
export function effectiveTaper({ min, max, taper = 'linear' }) {
  if (taper === 'log' && min > 0 && max > min) return 'log';
  if (taper === 'db' && min >= 0 && max > min) return 'db';
  return 'linear';
}

/** The dB at the bottom of a dB taper: the floor, or `min` in dB if higher. */
function dbBottom({ min, floorDb = DEFAULT_FLOOR_DB }) {
  return min > 0 ? Math.max(floorDb, 20 * Math.log10(min)) : floorDb;
}

/**
 * A value's position. Values are clamped to the range first.
 *
 * Under the `db` taper, positions above 0 are linear in dB from the floor
 * (`floorDb`, default −60 dB, or `min` in dB when that is higher) to
 * `max` in dB, and position 0 is `min`. So every value at or below the
 * floor's value (for `min` = 0: 0 < value <= 10^(−60/20)) maps to position
 * 0, and back to `min`: the round trip value → position → value holds only
 * for values above the floor (or exactly `min`).
 */
export function toPosition(value, range) {
  const { min, max } = range;
  if (!(max > min) || !Number.isFinite(value)) return 0;
  const v = clamp(value, min, max);
  switch (effectiveTaper(range)) {
    case 'log':
      return Math.log(v / min) / Math.log(max / min);
    case 'db': {
      if (v <= 0) return 0;
      const bottom = dbBottom(range);
      const top = 20 * Math.log10(max);
      return clamp((20 * Math.log10(v) - bottom) / (top - bottom), 0, 1);
    }
    default:
      return (v - min) / (max - min);
  }
}

/** A position's value (positions are clamped to 0..1). */
export function toValue(position, range) {
  const { min, max } = range;
  if (!(max > min)) return min;
  const p = clamp(Number.isFinite(position) ? position : 0, 0, 1);
  if (p === 0) return min;
  if (p === 1) return max;
  switch (effectiveTaper(range)) {
    case 'log':
      return min * Math.pow(max / min, p);
    case 'db': {
      const bottom = dbBottom(range);
      const top = 20 * Math.log10(max);
      return clamp(10 ** ((bottom + p * (top - bottom)) / 20), min, max);
    }
    default:
      return min + p * (max - min);
  }
}

/**
 * Where a bipolar control's arc starts: the position of 0 when the range
 * spans it, else the middle.
 */
export function bipolarOrigin(range) {
  return range.min < 0 && range.max > 0 ? toPosition(0, range) : 0.5;
}
