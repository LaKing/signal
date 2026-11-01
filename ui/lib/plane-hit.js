// Hit-testing and culling for the plane's Wiring and Control drawings (ADR
// 0051 phase 2; ADR 0047 phase 4: every drawn element is hit). A drawing is
// a list of items in plane units: a pin or a via (`{kind, id, x, y}`), a
// wire or a cable (`{kind, id, points}`, an orthogonal polyline) or a box
// (`{kind, id, x, y, w, h}`). A point hits the nearest pin within PIN_PX
// on screen, else the nearest wire within WIRE_PX, else the topmost box
// under it: pins win over wires, wires over boxes, as on the board. The
// tolerances are screen pixels, so a pin is as easy to hit at every zoom.
// Pure; no DOM.

/** A pin's or a via's reach, and a wire's, in screen px. */
export const PIN_PX = 9;
export const WIRE_PX = 5;

/** The distance from `p` to segment `a`–`b`. */
export function segmentDistance(p, a, b) {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const l = dx * dx + dy * dy;
  const t = l > 0 ? Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / l)) : 0;
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

/** The distance from `p` to polyline `points`. */
export function polylineDistance(p, points) {
  let d = Infinity;
  for (let i = 1; i < points.length; i++) d = Math.min(d, segmentDistance(p, points[i - 1], points[i]));
  if (points.length === 1) d = Math.hypot(p.x - points[0].x, p.y - points[0].y);
  return d;
}

/** An item's bounding box (plane units). */
export function itemBox(item) {
  if (item.points) {
    const xs = item.points.map((q) => q.x);
    const ys = item.points.map((q) => q.y);
    const x = Math.min(...xs);
    const y = Math.min(...ys);
    return { x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y };
  }
  if (item.w !== undefined) return { x: item.x, y: item.y, w: item.w, h: item.h };
  return { x: item.x, y: item.y, w: 0, h: 0 };
}

/**
 * The item of `items` at plane point `p`, the view at scale `k` (screen
 * px per plane unit): `{item, distance}` or null. Pins (no `points`, no
 * `w`) first, then wires, then boxes, the last box drawn on top.
 */
export function hitItem(items, p, k) {
  const pinReach = PIN_PX / k;
  const wireReach = WIRE_PX / k;
  let pin = null;
  let wire = null;
  let box = null;
  for (const item of items) {
    if (item.points) {
      const d = polylineDistance(p, item.points);
      if (d <= wireReach && (!wire || d < wire.distance)) wire = { item, distance: d };
    } else if (item.w !== undefined) {
      if (p.x >= item.x && p.x <= item.x + item.w && p.y >= item.y && p.y <= item.y + item.h) box = { item, distance: 0 };
    } else {
      const d = Math.hypot(p.x - item.x, p.y - item.y);
      if (d <= pinReach && (!pin || d < pin.distance)) pin = { item, distance: d };
    }
  }
  return pin ?? wire ?? box;
}

/**
 * Culling: the ids of the items whose box meets `view` (a rectangle in
 * plane units) grown by `margin`. What is drawn is only what is on the
 * stage; the rest keeps its element, hidden.
 */
export function visibleIds(items, view, margin = 0) {
  const r = { x: view.x - margin, y: view.y - margin, w: view.w + 2 * margin, h: view.h + 2 * margin };
  const out = new Set();
  for (const item of items) {
    const b = itemBox(item);
    if (b.x <= r.x + r.w && r.x <= b.x + b.w && b.y <= r.y + r.h && r.y <= b.y + b.h) out.add(item.id);
  }
  return out;
}
