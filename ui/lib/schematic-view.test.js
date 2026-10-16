import test from 'node:test';
import assert from 'node:assert/strict';
import {
  IDENTITY, MAX_SCALE, MIN_SCALE, compose, fit, focus, keyView, pan, screenRect, toLocal, toScreen, wheelFactor, zoomAt,
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

test('wheel and keys', () => {
  assert.ok(wheelFactor(-100) > 1 && wheelFactor(100) < 1, 'wheel up zooms in');
  assert.ok(wheelFactor(-10, { pinch: true }) > wheelFactor(-10), 'a pinch step zooms more per pixel');
  close(wheelFactor(3, { deltaMode: 1 }), wheelFactor(48), 'lines are 16 pixels');
  const viewport = { w: 400, h: 200 };
  const bounds = { x: 0, y: 0, w: 100, h: 50 };
  assert.ok(keyView(IDENTITY, '+', viewport, bounds).k > 1);
  assert.ok(keyView(IDENTITY, '−', viewport, bounds).k < 1);
  assert.deepEqual(keyView(IDENTITY, '0', viewport, bounds), fit(bounds, viewport));
  assert.deepEqual(keyView(IDENTITY, 'ArrowLeft', viewport, bounds), { k: 1, x: 40, y: 0 });
  assert.deepEqual(keyView(IDENTITY, 'ArrowDown', viewport, bounds), { k: 1, x: 0, y: -20 });
  assert.equal(keyView(IDENTITY, 'a', viewport, bounds), null);
});
