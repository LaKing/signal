// A board wire's route following a dragged chip (ADR 0047 phase 4; the
// owner: "when I drag an element the wiring under it gets redrawn"): the
// route kept, stretched at its moved end, so nothing jumps while the chip
// moves; the release reroutes only the chip's own wires
// (ui/lib/board-reroute.js). Pure; no DOM.

import { simplify } from './board-route.js';

/** Segment i's orientation in `c`: 'h', 'v', or for a point repeated the turn its neighbours make (perpendicular to the one before). */
function orient(c, i) {
  const p = c[i];
  const q = c[i + 1];
  if (p.y === q.y && p.x !== q.x) return 'h';
  if (p.x === q.x && p.y !== q.y) return 'v';
  return i > 0 ? (orient(c, i - 1) === 'h' ? 'v' : 'h') : 'v';
}

/** `c` with at least four corners: a repeated point at the middle of its first segment (a jog that can open). */
function expand(c) {
  if (c.length >= 4) return c.map((p) => ({ ...p }));
  const m = { x: (c[0].x + c[1].x) / 2, y: (c[0].y + c[1].y) / 2 };
  return [{ ...c[0] }, { ...m }, { ...m }, ...c.slice(1).map((p) => ({ ...p }))];
}

/** Orthogonal corners `c` with their last point moved by `d`: the last two corners ride, the one before slides along its segment. */
function moveEnd(c0, d) {
  const c = expand(c0);
  const n = c.length;
  const was = orient(c, n - 3);
  c[n - 1] = { x: c[n - 1].x + d.dx, y: c[n - 1].y + d.dy };
  c[n - 2] = { x: c[n - 2].x + d.dx, y: c[n - 2].y + d.dy };
  if (was === 'v') c[n - 3] = { x: c[n - 2].x, y: c[n - 3].y };
  else c[n - 3] = { x: c[n - 3].x, y: c[n - 2].y };
  return c;
}

/**
 * Route `corners` (orthogonal, from its output to its input) with its start
 * moved by `da` and its end by `db` (`{dx, dy}` or null): moved together,
 * the whole route; else each moved end stretched. Orthogonal, deterministic.
 */
export function stretchCorners(corners, da, db) {
  if (!corners?.length) return corners;
  const zero = (d) => !d || (d.dx === 0 && d.dy === 0);
  if (zero(da) && zero(db)) return corners;
  if (corners.length < 2) return corners.map((p) => ({ x: p.x + (da?.dx ?? db.dx), y: p.y + (da?.dy ?? db.dy) }));
  if (!zero(da) && !zero(db) && da.dx === db.dx && da.dy === db.dy) return corners.map((p) => ({ x: p.x + da.dx, y: p.y + da.dy }));
  let c = corners;
  if (!zero(db)) c = moveEnd(c, db);
  if (!zero(da)) c = moveEnd([...c].reverse(), da).reverse();
  return simplify(c);
}
