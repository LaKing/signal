import test from 'node:test';
import assert from 'node:assert/strict';
import {
  IDENTITY, MAX_SCALE, MIN_SCALE, commandView, compose, fit, focus, pan, screenRect, toLocal, toScreen, uncompose, wheelFactor, zoomAt,
} from './schematic-view.js';

const close = (a, b, message) => assert.ok(Math.abs(a - b) <= 1e-9 * Math.max(1, Math.abs(a), Math.abs(b)), `${message}: ${a} vs ${b}`);

test('zooming keeps the point under the cursor where it is', () => {
  const view = { k: 2, x: 30, y: -10 };
  const cursor = { x: 123, y: 45 };
  const under = toLocal(view, cursor);
  for (const factor of [1.5, 0.3, 10, 1 / 7]) {
    const zoomed = zoomAt(view, factor, cursor);
    close(zoomed.k, 2 * factor, 'scale');
    const back = toScreen(zoomed, under);
    close(back.x, cursor.x, 'x');
    close(back.y, cursor.y, 'y');
  }
});

test('pan moves by screen pixels and fit centres a rectangle', () => {
  assert.deepEqual(pan(IDENTITY, 5, -3), { k: 1, x: 5, y: -3 });
  const view = fit({ x: 100, y: 0, w: 200, h: 100 }, { w: 448, h: 448 }, 24);
  close(view.k, 2, 'the wider side fills');
  const centre = toScreen(view, { x: 200, y: 50 });
  close(centre.x, 224, 'centred x');
  close(centre.y, 224, 'centred y');
});

test('focus makes a part as drawn fill the viewport', () => {
  const view = { k: 3, x: 10, y: 20 };
  const part = { x: 0, y: 0, w: 40, h: 20 };
  const focused = focus(view, screenRect(view, part), { w: 448, h: 448 }, 24);
  const shown = screenRect(focused, part);
  close(shown.w, 400, 'the part fills the width');
  close(shown.x, 24, 'from the margin');
});

test('compose maps a level inside a box to the screen', () => {
  const view = { k: 2, x: 5, y: 7 };
  const inner = { k: 0.25, x: 100, y: 40 };
  const both = compose(view, inner);
  const point = { x: 8, y: 4 };
  const parent = toScreen(inner, point);
  const direct = toScreen(view, parent);
  const composed = toScreen(both, point);
  close(composed.x, direct.x, 'x');
  close(composed.y, direct.y, 'y');
});

test('the zoom is effectively unbounded, and clamped only to stay finite', () => {
  let view = IDENTITY;
  for (let i = 0; i < 400; i++) view = zoomAt(view, 10, { x: 50, y: 50 });
  assert.equal(view.k, MAX_SCALE);
  assert.ok(Number.isFinite(view.x) && Number.isFinite(view.y));
  for (let i = 0; i < 800; i++) view = zoomAt(view, 0.1, { x: 50, y: 50 });
  assert.equal(view.k, MIN_SCALE);
  assert.ok(Number.isFinite(view.x) && Number.isFinite(view.y));
  // Deep but ordinary zoom keeps exact numbers: 1e12 is no limit.
  assert.equal(zoomAt({ k: 1e12, x: 0, y: 0 }, 2, { x: 0, y: 0 }).k, 2e12);
  assert.deepEqual(pan(view, Infinity, 0), view, 'a non-finite step changes nothing');
});

// auditor-1-zoom-clamp: no clamp on the camera. A point at x = 1e5 under
// the pointer at k = 100 (translation −9 999 500) stays under it, zoom
// after zoom on a level with nothing nested, and its neighbours follow the
// exact scaling law.
test('zooming about the pointer keeps its point there, at any translation, on a terminal level', () => {
  const at = { x: 500, y: 300 };
  let view = { k: 100, x: -9999500, y: 300 };
  const point = { x: 1e5, y: 0 };
  close(toScreen(view, point).x, 500, 'under the pointer');
  // Two floats from it: `near.x − 1e5` is exact, so its true offset is that times k.
  const near = { x: 1e5 + 3e-11, y: 0 };
  for (let n = 1; n <= 120; n++) {
    view = zoomAt(view, 1.25, at);
    assert.ok(Math.abs(toScreen(view, point).x - at.x) < 1e-6, `step ${n}: the point stays`);
    const offset = toScreen(view, near).x - at.x;
    assert.ok(Math.abs(offset - (near.x - 1e5) * view.k) < 0.5, `step ${n}: a neighbour at ${offset} px`);
  }
  assert.ok(view.k > 1e13, `zoomed to ${view.k}`);
});

// Levels on either side of a link agree on screen at any scale, so the view
// can move between them (re-rooting) without anything moving.
test('compose and uncompose agree to a hundredth of a pixel at any scale', () => {
  const inner = { k: 6.3e-5, x: 1008.5, y: 466864.25 };
  const parentEdge = { x: 1000, y: 466875 };
  const childPoint = { x: 123.5, y: 456.75 };
  for (const k of [21, 1e6, 1e12]) {
    const parent = { k, ox: parentEdge.x, oy: parentEdge.y, x: 500, y: 300 };
    const child = compose(parent, inner);
    const back = uncompose(child, inner);
    for (const [a, b] of [[toScreen(parent, parentEdge), toScreen(back, parentEdge)], [toScreen(child, childPoint), toScreen(compose(back, inner), childPoint)]]) {
      assert.ok(Math.abs(a.x - b.x) < 0.01 && Math.abs(a.y - b.y) < 0.01, `k ${k}: ${a.x},${a.y} vs ${b.x},${b.y}`);
    }
    close(child.k, k * inner.k, 'scales multiply');
  }
});

test('wheel and keys', () => {
  assert.ok(wheelFactor(-100) > 1 && wheelFactor(100) < 1, 'wheel up zooms in');
  assert.ok(wheelFactor(-10, { pinch: true }) > wheelFactor(-10), 'a pinch step zooms more per pixel');
  close(wheelFactor(3, { deltaMode: 1 }), wheelFactor(48), 'lines are 16 pixels');
  const viewport = { w: 400, h: 200 };
  const bounds = { x: 0, y: 0, w: 100, h: 50 };
  assert.ok(commandView(IDENTITY, 'schematic.zoom_in', viewport, bounds).k > 1);
  assert.ok(commandView(IDENTITY, 'schematic.zoom_out', viewport, bounds).k < 1);
  assert.deepEqual(commandView(IDENTITY, 'schematic.fit', viewport, bounds), fit(bounds, viewport));
  assert.deepEqual(commandView(IDENTITY, 'schematic.pan_left', viewport, bounds), { k: 1, x: 40, y: 0 });
  assert.deepEqual(commandView(IDENTITY, 'schematic.pan_down', viewport, bounds), { k: 1, x: 0, y: -20 });
  assert.deepEqual(commandView(IDENTITY, 'schematic.page_down', viewport, bounds), { k: 1, x: 0, y: -viewport.h * 0.875 });
  assert.equal(commandView(IDENTITY, 'schematic.trace', viewport, bounds), null);
});

test('a fit can keep clear of a bar at the top and of labels at the sides', () => {
  const rect = { x: 0, y: 0, w: 1000, h: 1000 };
  const plain = fit(rect, { w: 1000, h: 1000 });
  assert.equal(plain.y, 500);
  const barred = fit(rect, { w: 1000, h: 1000 }, 24, 40);
  // The rectangle's top edge lands at the bar's bottom, its bottom edge at the margin.
  assert.ok(Math.abs(barred.y - barred.k * 500 - 40) < 1e-9);
  assert.ok(Math.abs(barred.y + barred.k * 500 - (1000 - 24)) < 1e-9);
  const sided = fit({ x: 0, y: 0, w: 1000, h: 10 }, { w: 1000, h: 1000 }, 24, 24, 120);
  assert.ok(Math.abs(sided.k - 760 / 1000) < 1e-9);
});
