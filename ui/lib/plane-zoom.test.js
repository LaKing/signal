// ADR 0051 §8, test 4: the plane's levels and fades. Band opacities are
// monotonic and exactly 0 and 1 at their ends; the level follows the ratio
// to the Stack's fit; focus fits the width and the wheel scrolls within the
// Stack; zooming about a point keeps it; the board's view maps both ways.

import test from 'node:test';
import assert from 'node:assert/strict';
import { FOCUS_BAND, MARGIN_PX, MAX_R, MIN_R, SURROUND, WIRING_BAND, bandShare, boardOriginY, boardView, clampScroll, fitAll, fitScale, fitWidth, fromBoardView, levelOf, planeFades, reachesFit, screenRect, scrollStack, toPlane, toScreen, zoomAbout } from './plane-zoom.js';

const close = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) <= eps, `${a} ≈ ${b}`);

test('a band’s share is exactly 0 and 1 at its ends, monotonic between', () => {
  for (const band of [FOCUS_BAND, WIRING_BAND]) {
    assert.equal(bandShare(band[0], band), 0);
    assert.equal(bandShare(band[1], band), 1);
    assert.equal(bandShare(band[0] - 0.1, band), 0);
    assert.equal(bandShare(band[1] + 10, band), 1);
    let last = 0;
    for (let i = 0; i <= 200; i++) {
      const r = band[0] + ((band[1] - band[0]) * i) / 200;
      const s = bandShare(r, band);
      assert.ok(s >= last, `monotonic at ${r}`);
      assert.ok(s >= 0 && s <= 1);
      last = s;
    }
  }
  assert.equal(bandShare(Number.NaN, FOCUS_BAND), 0);
});

test('the fades: Setup things to SURROUND over the focus band, the panels out and the board in over the wiring band, one curve both ways', () => {
  assert.deepEqual(planeFades(0.1), { focus: 0, wiring: 0, setup: 1, panels: 1, deep: 0 });
  const fit = planeFades(1);
  assert.equal(fit.setup, SURROUND);
  assert.equal(fit.panels, 1);
  assert.equal(fit.deep, 0);
  const past = planeFades(WIRING_BAND[1]);
  assert.equal(past.panels, 0);
  assert.equal(past.deep, 1);
  // Monotonic over the whole range: setup and panels never rise, deep never falls.
  let prev = planeFades(0.01);
  for (let r = 0.01; r <= 3; r += 0.005) {
    const f = planeFades(r);
    assert.ok(f.setup <= prev.setup + 1e-12 && f.panels <= prev.panels + 1e-12 && f.deep >= prev.deep - 1e-12, `at ${r}`);
    close(f.panels + f.deep, 1);
    prev = f;
  }
  // No timer: the same ratio is the same picture, zooming in or out.
  assert.deepEqual(planeFades(1.7), planeFades(1.7));
});

test('the level follows the ratio to the fit', () => {
  assert.equal(levelOf(0.5), 'setup');
  assert.equal(levelOf(0.9999), 'setup');
  assert.equal(levelOf(1), 'stack');
  assert.equal(levelOf(1 - 1e-9), 'stack');
  assert.equal(levelOf(1.6), 'stack');
  assert.equal(levelOf(WIRING_BAND[1]), 'wiring');
  assert.ok(reachesFit(0.9, 1.1));
  assert.ok(!reachesFit(1, 1.2));
  assert.ok(!reachesFit(0.5, 0.8));
});

test('focus fits the Stack’s width; the wheel then scrolls it, its top and bottom kept on the stage', () => {
  const stack = { x: 100, y: 40, w: 1280, h: 5000 };
  const viewport = { w: 1328, h: 800 };
  const view = fitWidth(stack, viewport);
  close(view.k, (1328 - 2 * MARGIN_PX) / 1280);
  const r = screenRect(view, stack);
  close(r.x, MARGIN_PX);
  close(r.x + r.w, viewport.w - MARGIN_PX);
  close(r.y, MARGIN_PX);
  close(fitScale(stack, viewport), view.k);
  // Down a page, then far past the end: the bottom stops at the margin.
  const down = scrollStack(view, 700, stack, viewport);
  close(screenRect(down, stack).y, MARGIN_PX - 700);
  const end = scrollStack(down, 1e6, stack, viewport);
  close(screenRect(end, stack).y + screenRect(end, stack).h, viewport.h - MARGIN_PX);
  const top = scrollStack(end, -1e6, stack, viewport);
  close(screenRect(top, stack).y, MARGIN_PX);
  // The scale and x never change with a scroll: no rectangle moves sideways.
  assert.equal(end.k, view.k);
  assert.equal(end.x, view.x);
  // A short Stack stays at the top.
  const short = { x: 0, y: 0, w: 1280, h: 300 };
  close(screenRect(clampScroll({ k: 1, x: 0, y: 500 }, short, viewport), short).y, MARGIN_PX);
  // Fitting again from a scrolled view keeps the point at the centre.
  const kept = fitWidth(stack, viewport, down);
  close(toPlane(kept, { x: 0, y: viewport.h / 2 }).y, toPlane(down, { x: 0, y: viewport.h / 2 }).y, 1e-6);
});

test('zooming about a point keeps it; the ratio stays within its limits', () => {
  const view = { k: 0.5, x: 10, y: -20 };
  const at = { x: 300, y: 200 };
  const p = toPlane(view, at);
  const next = zoomAbout(view, 1.7, at);
  const q = toScreen(next, p);
  close(q.x, at.x);
  close(q.y, at.y);
  const capped = zoomAbout(view, 1e9, at, 1);
  close(capped.k, MAX_R);
  assert.equal(zoomAbout(view, Number.NaN, at), view);
});

test('fit all shows the bounds whole, centred', () => {
  const bounds = { x: -500, y: 0, w: 3000, h: 4000 };
  const viewport = { w: 1000, h: 800 };
  const v = fitAll(bounds, viewport);
  const r = screenRect(v, bounds);
  assert.ok(r.x >= MARGIN_PX - 1e-9 && r.y >= MARGIN_PX - 1e-9);
  assert.ok(r.x + r.w <= viewport.w - MARGIN_PX + 1e-9 && r.y + r.h <= viewport.h - MARGIN_PX + 1e-9);
  close(r.x + r.w / 2, viewport.w / 2);
});

test('the board’s view lays its plate origin on the Stack’s panels, and maps back', () => {
  const view = { k: 1.8, x: -40, y: -900 };
  const origin = { x: 0, y: 56 };
  const offset = { x: 300, y: 36 };
  const board = boardView(view, origin, offset);
  // The board's (0, 0) is drawn where the plane's origin point is.
  close(board.x + offset.x, toScreen(view, origin).x);
  close(board.y + offset.y, toScreen(view, origin).y);
  const back = fromBoardView(board, origin, offset);
  close(back.k, view.k);
  close(back.x, view.x);
  close(back.y, view.y);
  // A board view kept about another origin (re-rooted) maps back the same.
  const moved = { k: board.k, ox: 100, oy: 50, x: board.x + 100 * board.k, y: board.y + 50 * board.k };
  const again = fromBoardView(moved, origin, offset);
  close(again.x, view.x, 1e-9);
  close(again.y, view.y, 1e-9);
});

test('the fades are continuous across both bands: no step anywhere, the steepest change the smoothstep’s', () => {
  // A smoothstep over [a, b] changes by at most 1.5 / (b − a) per unit of r; sampled finely, no step exceeds that slope.
  const slope = Math.max(1.5 / (FOCUS_BAND[1] - FOCUS_BAND[0]), 1.5 / (WIRING_BAND[1] - WIRING_BAND[0]));
  const dr = 1e-4;
  let prev = planeFades(MIN_R);
  for (let r = MIN_R + dr; r <= MAX_R + 0.5; r += dr) {
    const f = planeFades(r);
    for (const key of ['setup', 'panels', 'deep']) assert.ok(Math.abs(f[key] - prev[key]) <= slope * dr * 1.0001 + 1e-12, `${key} steps at r ${r}`);
    prev = f;
  }
  // The band ends meet their neighbours exactly: the level's change at the fit and at the band's end shows nothing new.
  assert.equal(planeFades(FOCUS_BAND[0]).setup, 1);
  assert.equal(planeFades(1 - 1e-12).setup, planeFades(1).setup);
  assert.equal(planeFades(WIRING_BAND[0]).deep, 0);
  assert.equal(planeFades(WIRING_BAND[1] - 1e-12).panels, planeFades(WIRING_BAND[1]).panels);
  assert.equal(planeFades(WIRING_BAND[1]).panels, 0);
});

test('the board’s origin follows the card at the view’s centre, continuously between cards', () => {
  const anchors = [
    { y: 200, origin: -4 },
    { y: 900, origin: -2 },
    { y: 2600, origin: 4 },
  ];
  assert.equal(boardOriginY([], 500, 76), 76);
  assert.equal(boardOriginY(anchors, 0, 76), -4);
  assert.equal(boardOriginY(anchors, 200, 76), -4);
  assert.equal(boardOriginY(anchors, 900, 76), -2);
  close(boardOriginY(anchors, 550, 76), -3);
  assert.equal(boardOriginY(anchors, 9000, 76), 4);
  // Continuous and monotonic between anchors that rise: a scroll never moves the board by a jump.
  let prev = boardOriginY(anchors, -100, 76);
  for (let y = -100; y <= 3000; y += 0.5) {
    const o = boardOriginY(anchors, y, 76);
    assert.ok(o >= prev - 1e-12 && o - prev <= (6 / 1700) * 0.5 + 1e-9, `at ${y}`);
    prev = o;
  }
});
