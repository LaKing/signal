import test from 'node:test';
import assert from 'node:assert/strict';
import {
  CLIP_PX, HUGE_PIN_PX, SVG_MAX_SCALE, SVG_MAX_TRANSLATE, SVG_MIN_SCALE,
  bake, bakeHolds, bakeScale, bakedMatrix, bakedPath, bakedPin, bakedPoint, bakedRect, inBounds, reconcile, sameItems,
} from './schematic-draw.js';
import { layout } from './schematic-layout.js';
import { scene } from './schematic-scene.js';
import { fit, pan, toScreen, zoomAt } from './schematic-view.js';
import { KICK } from './schematic-scene.test.js';

/** Every matrix handed to SVG is within the bounds (AC16). */
function bounded(matrix) {
  assert.ok(matrix.k >= SVG_MIN_SCALE && matrix.k <= SVG_MAX_SCALE, `scale ${matrix.k}`);
  assert.ok(Math.abs(matrix.x) <= SVG_MAX_TRANSLATE && Math.abs(matrix.y) <= SVG_MAX_TRANSLATE, `translation ${matrix.x}, ${matrix.y}`);
}

/** A baked point on the screen through `matrix`. */
const device = (matrix, p) => ({ x: p.x * matrix.k + matrix.x, y: p.y * matrix.k + matrix.y });

test('the bake factor keeps every scale within [1e-6, 1e6]', () => {
  for (let e = -12; e <= 14; e += 0.05) {
    const k = 10 ** e;
    const m = bakeScale(k);
    assert.ok(k / m >= SVG_MIN_SCALE && k / m <= SVG_MAX_SCALE, `k ${k}: k/m ${k / m}`);
  }
  assert.equal(bakeScale(1), 1, 'ordinary scales are not baked');
  assert.equal(bakeScale(1e6), 1);
  assert.equal(bakeScale(1e-6), 1);
});

// auditor-1-svg-small-scale: a wire of 1e9 units at k = 5e-7 is 500 px on
// screen: drawn whole, under a matrix within the bounds.
test('a long wire at a small scale is drawn over its whole extent, within bounds', () => {
  const wire = { id: 0, shape: 'signal', points: [{ x: 0, y: 0 }, { x: 0, y: 1e9 }] };
  const level = { nodes: [], wires: [wire], bounds: { x: 0, y: 0, w: 1, h: 1e9 } };
  const viewport = { w: 1000, h: 600 };
  const view = { k: 5e-7, ox: 0, oy: 5e8, x: 500, y: 300 };
  const drawn = scene({ layoutOf: () => level, view, viewport });
  assert.deepEqual(drawn.levels[0].wires, [wire], 'not culled');
  const baked = bake(view, viewport);
  assert.equal(baked.m, 0.1);
  const matrix = bakedMatrix(baked, view);
  bounded(matrix);
  assert.ok(Math.abs(matrix.k - 5e-6) < 1e-18);
  const d = bakedPath(baked, wire.points);
  const [a, b] = d.match(/-?[\d.e+-]+,-?[\d.e+-]+/g).map((s) => s.split(',').map(Number)).map(([x, y]) => device(matrix, { x, y }));
  const [ta, tb] = wire.points.map((p) => toScreen(view, p));
  for (const [got, want] of [[a, ta], [b, tb]]) {
    assert.ok(Math.abs(got.x - want.x) < 0.01 && Math.abs(got.y - want.y) < 0.01, `${JSON.stringify(got)} vs ${JSON.stringify(want)}`);
  }
  assert.ok(Math.abs(b.y - a.y - 500) < 0.01, 'all 500 px of it');
  // Smaller still: in bounds and whole wherever it spans 2 px or more.
  for (let k = 1e-9; k <= 1e-6; k *= 1.7) {
    const v = { k, ox: 0, oy: 5e8, x: 500, y: 300 };
    const kept = scene({ layoutOf: () => level, view: v, viewport }).levels[0].wires.length === 1;
    assert.equal(kept, 1e9 * k >= 2, `k ${k}`);
    const bk = bake(v, viewport);
    bounded(bakedMatrix(bk, v));
    assert.ok(!bakedPath(bk, wire.points).includes('M') || !bk.clipped, `k ${k}: not clipped`);
  }
});

// auditor-1-giant-text-removal: text is drawn whenever it is at least 4 px,
// under a bounded matrix; at k = 1e5 a name is 1.2e6 px, as it always was.
test('giant text keeps its size under a bounded matrix', () => {
  const viewport = { w: 1000, h: 600 };
  const node = { x: 100, y: 200, w: 117, h: 90 };
  const anchor = { x: node.x + 8, y: node.y + 14 };
  for (const [k, m] of [[1e5, 1], [1e9, 1e3]]) {
    const view = { k, ox: anchor.x + 1e-4, oy: anchor.y - 1e-4, x: 500, y: 300 };
    const baked = bake(view, viewport);
    assert.equal(baked.m, m, `k ${k}`);
    const matrix = bakedMatrix(baked, view);
    bounded(matrix);
    assert.ok(12 * k >= 4, 'past the 4 px minimum, so the name is emitted');
    // font-size 12·m under scale k/m: 12·k on screen, the anchor where it was.
    assert.ok(Math.abs(12 * baked.m * matrix.k - 12 * k) <= 1e-9 * 12 * k);
    const at = device(matrix, bakedPoint(baked, anchor));
    const want = toScreen(view, anchor);
    assert.ok(Math.abs(at.x - want.x) < 0.01 && Math.abs(at.y - want.y) < 0.01);
    assert.ok(Number.isFinite(bakedPoint(baked, anchor).x));
  }
});

/** Whether `p` is inside polygon `poly` (even-odd). */
function insidePolygon(p, poly) {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const a = poly[i];
    const b = poly[j];
    if (a.y > p.y !== b.y > p.y && p.x < ((b.x - a.x) * (p.y - a.y)) / (b.y - a.y) + a.x) inside = !inside;
  }
  return inside;
}

/**
 * The huge pin drawn by `baked` (built for `built`) as it shows under
 * `transform`: it covers the viewport (plus 16 px) where the disc does and
 * nowhere the disc does not, to 0.01 px, and every vertex is in bounds.
 */
function pinShows(baked, polygon, transform, viewport, pin, radius) {
  const matrix = bakedMatrix(baked, transform);
  bounded(matrix);
  const poly = polygon.map((p) => device(matrix, p));
  const c = toScreen(transform, pin);
  const r = radius * (transform.k / baked.cover.k);
  for (const p of polygon) assert.ok(Math.abs(p.x * baked.m) < 1e13 && Number.isFinite(p.y));
  for (let i = 0; i <= 40; i++) {
    for (let j = 0; j <= 40; j++) {
      const p = { x: -16 + ((viewport.w + 32) * i) / 40, y: -16 + ((viewport.h + 32) * j) / 40 };
      const d = Math.hypot(p.x - c.x, p.y - c.y);
      if (d < r - 0.01) assert.ok(insidePolygon(p, poly), `uncovered at ${p.x}, ${p.y}`);
      if (d > r + 0.01) assert.ok(!insidePolygon(p, poly), `drawn outside the disc at ${p.x}, ${p.y}`);
    }
  }
}

// auditor-1-pin-clip-cache-invalidation, auditor-2-huge-pin-rebake-trigger.
test('a huge pin is the disc near the screen, baked again only when the view leaves its margin or doubles', () => {
  const viewport = { w: 1000, h: 1000 };
  for (const radius of [HUGE_PIN_PX + 1, 1e12]) {
    const k = radius / 3;
    const pin = { x: 0, y: 0 };
    // Its centre at (500, 300 + radius): the disc's top edge crosses the viewport at y = 300.
    const view = { k, ox: 0, oy: 0, x: 500, y: 300 + radius };
    const baked = bake(view, viewport);
    const shape = bakedPin(baked, view, viewport, pin);
    assert.ok(shape.polygon?.length > 3, 'a polygon, not a circle');
    pinShows(baked, shape.polygon, view, viewport, pin, radius);
    // A 50 px pan: no new bake, still covered (auditor-1's case).
    let moved = pan(view, 50, 50);
    assert.ok(bakeHolds(baked, moved, viewport));
    pinShows(baked, shape.polygon, moved, viewport, pin, radius);
    // 1983 px: still within the margin (2 viewports less 16 px).
    moved = pan(view, 1983, 0);
    assert.ok(bakeHolds(baked, moved, viewport));
    pinShows(baked, shape.polygon, moved, viewport, pin, radius);
    // 1990 px: past it, baked again, and covered again.
    moved = pan(view, 1990, 0);
    assert.equal(bakeHolds(baked, moved, viewport), false);
    const again = bake(moved, viewport);
    pinShows(again, bakedPin(again, moved, viewport, pin).polygon, moved, viewport, pin, radius);
    // Zoomed in ×1.9 about the centre: still within 0.01 px; ×2.1: baked again.
    const centre = { x: 500, y: 500 };
    moved = zoomAt(view, 1.9, centre);
    assert.ok(bakeHolds(baked, moved, viewport));
    pinShows(baked, shape.polygon, moved, viewport, pin, radius);
    moved = zoomAt(view, 2.1, centre);
    assert.equal(bakeHolds(baked, moved, viewport), false);
    const deeper = bake(moved, viewport);
    pinShows(deeper, bakedPin(deeper, moved, viewport, pin).polygon, moved, viewport, pin, radius * 2.1);
    // Zoomed out ×0.5 the margin (5 viewports across) still holds the
    // screen; ×0.2 it no longer does.
    moved = zoomAt(view, 0.5, centre);
    assert.ok(bakeHolds(baked, moved, viewport));
    pinShows(baked, shape.polygon, moved, viewport, pin, radius);
    assert.equal(bakeHolds(baked, zoomAt(view, 0.2, centre), viewport), false);
  }
  // Below HUGE_PIN_PX a pin is a circle.
  const view = { k: 1, ox: 0, oy: 0, x: 500, y: 500 };
  assert.deepEqual(bakedPin(bake(view, viewport), view, viewport, { x: 10, y: 20 }), { circle: { cx: 10, cy: 20, r: 3 } });
});

// auditor-1-bake-origin-rounding: at k = 1e20 the point under the centre
// rounds back to the camera's origin, 2e7 px off screen; the offset goes
// into the coordinates and the matrix stays within the bounds.
test('a level whose content origin rounds far off screen still gets a bounded matrix', () => {
  const viewport = { w: 1000, h: 600 };
  const box = { id: 'p', name: 'p', kind: 'instance', composite: false, module: 'std.x', x: 1e5, y: 1e5, w: 100, h: 60, ins: [], outs: [] };
  const level = { nodes: [box], wires: [], bounds: { x: 1e5, y: 1e5, w: 100, h: 60 } };
  const view = { k: 1e20, ox: 1e5, oy: 1e5, x: -2e7, y: -2e7 };
  const drawn = scene({ layoutOf: () => level, view, viewport });
  assert.deepEqual(drawn.levels[0].nodes, [box]);
  assert.equal(drawn.levels[0].behind, false);
  const baked = bake(view, viewport);
  assert.deepEqual(baked.q, { x: 1e5, y: 1e5 }, 'q is the origin: nothing nearer is representable');
  assert.ok(baked.representable);
  const matrix = bakedMatrix(baked, view);
  bounded(matrix);
  assert.ok(inBounds(matrix));
  // The box covers the screen, as it does: its edge at −2e7 px is clipped off screen.
  const rect = bakedRect(baked, box);
  const a = device(matrix, { x: rect.x, y: rect.y });
  const b = device(matrix, { x: rect.x + rect.width, y: rect.y + rect.height });
  assert.ok(a.x <= -16 && a.y <= -16 && b.x >= viewport.w + 16 && b.y >= viewport.h + 16, `${a.x},${a.y} … ${b.x},${b.y}`);
  assert.ok(Math.abs(rect.x) * matrix.k <= 1.1 * CLIP_PX && Math.abs(rect.width) * matrix.k <= 2.2 * CLIP_PX);
  // Past the camera's precision (k = 1e30: the level's nearest point to the
  // centre is 2e16 px away): not drawn at all.
  assert.equal(bake({ ...view, k: 1e30, x: -2e16, y: -2e16 }, viewport).representable, false);
});

test('geometry far off screen is clipped, and what is visible is not', () => {
  const viewport = { w: 1000, h: 600 };
  const view = { k: 100, ox: 0, oy: 0, x: 500, y: 300 };
  const baked = bake(view, viewport);
  const far = CLIP_PX / view.k;
  const rect = bakedRect(baked, { x: -10, y: -10, w: 3 * far, h: 20 });
  assert.ok(baked.clipped);
  const matrix = bakedMatrix(baked, view);
  const left = device(matrix, { x: rect.x, y: rect.y });
  assert.ok(Math.abs(left.x - toScreen(view, { x: -10, y: 0 }).x) < 1e-6, 'its visible edge is where it was');
  assert.ok(Math.abs(rect.width * matrix.k) <= 1.2 * CLIP_PX, 'its far edge is clipped');
  assert.equal(bakeHolds(baked, view, viewport), true);
});

test('elements are reused by key while their item and flags are unchanged', () => {
  const build = (item) => ({ built: item.id });
  const a = { id: 'a' };
  const b = { id: 'b' };
  const c = { id: 'c' };
  let result = reconcile(new Map(), [a, b], (i) => i.id, () => '', build);
  assert.deepEqual([result.created, result.removed], [2, 0]);
  const first = result.elements;
  // The same items: nothing built.
  result = reconcile(result.cache, [a, b], (i) => i.id, () => '', build);
  assert.equal(result.created, 0);
  assert.ok(sameItems(result.elements, first));
  // Culling: b gone, c in; a kept.
  result = reconcile(result.cache, [a, c], (i) => i.id, () => '', build);
  assert.deepEqual([result.created, result.removed], [1, 1]);
  assert.equal(result.elements[0], first[0]);
  // A new layout (new objects under the same keys), or new flags: built again.
  result = reconcile(result.cache, [{ id: 'a' }, c], (i) => i.id, () => '', build);
  assert.equal(result.created, 1);
  result = reconcile(result.cache, result.cache.size ? [...result.cache.values()].map((e) => e.item) : [], (i) => i.id, () => 'text', build);
  assert.equal(result.created, 2);
  assert.equal(sameItems([1, 2], [1, 2]), true);
  assert.equal(sameItems([1, 2], [2, 1]), false);
});

// AC1, AC2: a pan at Fit draws the same elements; only the matrices change.
test('a pan at Fit keeps every level’s elements and bake; only matrices move', () => {
  const layouts = new Map();
  const layoutOf = (level) => {
    if (!layouts.has(level)) layouts.set(level, layout(level === '' ? KICK.top : KICK.composites[level]));
    return layouts.get(level);
  };
  const viewport = { w: 1000, h: 600 };
  const view = fit(layoutOf('').bounds, viewport);
  // drums open (as a click opens it), so a pan moves two levels.
  const pinned = new Set(['/drums']);
  const before = scene({ layoutOf, view, viewport, pinned });
  const bakes = before.levels.map((l) => bake(l.transform, viewport));
  for (const dx of [1, 7, -30]) {
    const after = scene({ layoutOf, view: pan(before.view, dx, 0), viewport, pinned });
    assert.deepEqual(after.levels.map((l) => l.path), before.levels.map((l) => l.path), 'no level opened or closed');
    after.levels.forEach((level, i) => {
      assert.ok(sameItems(level.nodes, before.levels[i].nodes), 'the same nodes, element by element');
      assert.ok(sameItems(level.wires, before.levels[i].wires), 'the same wires');
      assert.ok(bakeHolds(bakes[i], level.transform, viewport), 'no new bake');
      const moved = bakedMatrix(bakes[i], level.transform);
      const was = bakedMatrix(bakes[i], before.levels[i].transform);
      assert.ok(Math.abs(moved.x - was.x - dx) < 1e-9 && moved.k === was.k, 'the matrix moved by the pan');
    });
  }
});
