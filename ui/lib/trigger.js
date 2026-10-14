// Scope triggering and X–Y pairing for <signal-scope>. Points are the scope
// stream's (docs/ui-protocol.md "Scope streams"): one every `decimation`
// frames, at absolute frames that are multiples of it.

/**
 * The index of the first point at or after `from` (and after the first
 * point) where the signal crosses `level` on `edge`: `rising` when the
 * previous point is below the level and this one at or above it, `falling`
 * the other way round. −1 when there is none.
 */
export function findTrigger(points, { level = 0, edge = 'rising', from = 1 } = {}) {
  for (let i = Math.max(1, from); i < points.length; i++) {
    const before = points[i - 1];
    const at = points[i];
    if (edge === 'falling' ? before > level && at <= level : before < level && at >= level) {
      return i;
    }
  }
  return -1;
}

/**
 * Where a time display of `length` points starts in `points`: the first
 * trigger that leaves `length` points after it, else 0 (free running).
 */
export function triggerStart(points, length, options = {}) {
  const at = findTrigger(points, options);
  return at >= 0 && at + length <= points.length ? at : 0;
}

/**
 * The common part of two scope blocks `{firstFrame, decimation, values}`
 * for an X–Y display: `{firstFrame, xs, ys}`, or null when the decimations
 * differ, the points do not fall on the same frames, or they do not
 * overlap.
 */
export function alignXY(x, y) {
  if (!x || !y || x.decimation !== y.decimation || x.decimation <= 0) return null;
  const offset = (y.firstFrame - x.firstFrame) / x.decimation;
  if (!Number.isInteger(offset)) return null;
  const xStart = Math.max(0, offset);
  const yStart = Math.max(0, -offset);
  const count = Math.min(x.values.length - xStart, y.values.length - yStart);
  if (count <= 0) return null;
  return {
    firstFrame: x.firstFrame + xStart * x.decimation,
    xs: x.values.slice(xStart, xStart + count),
    ys: y.values.slice(yStart, yStart + count),
  };
}
