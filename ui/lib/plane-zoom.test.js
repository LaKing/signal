// ADR 0051 §8, test 4: the plane's levels and fades. Band opacities are
// monotonic and exactly 0 and 1 at their ends; the level follows the ratio
// to the Stack's fit; focus fits the width and the wheel scrolls within the
// Stack; zooming about a point keeps it; phase 2: the Wiring, Control and
// Engine bands nest, each monotonic across its band, and the Engine's view
// lays the board's frame on the cube both ways.

import test from 'node:test';
import assert from 'node:assert/strict';
import { CONTROL_BAND, ENGINE_BAND, ENGINE_GAP, FOCUS_BAND, LIFT, MARGIN_PX, depthOf, splitDepth, MAX_R, MIN_R, SURROUND, WIRING_BAND, bandShare, clampScroll, engineBand, fitAll, fitScale, fitWidth, frameOnto, instrumentAt, levelOf, opensAt, planeFades, planeFromFrame, reachesFit, screenRect, scrollStack, toPlane, toScreen, zoomAbout, zoomStep } from './plane-zoom.js';

const close = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) <= eps, `${a} ≈ ${b}`);

test('a band’s share is exactly 0 and 1 at its ends, monotonic between', () => {
  for (const band of [FOCUS_BAND, WIRING_BAND, CONTROL_BAND, ENGINE_BAND, engineBand(9)]) {
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

test('the fades: Setup things to SURROUND over the focus band, the panels out and the Wiring in over the wiring band, one curve both ways', () => {
  const low = planeFades(0.1);
  assert.equal(low.setup, 1);
  assert.equal(low.panels, 1);
  assert.equal(low.board, 0);
  assert.equal(low.sheet, 0);
  assert.equal(low.deep, 0);
  const fit = planeFades(1);
  assert.equal(fit.setup, SURROUND);
  assert.equal(fit.panels, 1);
  assert.equal(fit.board, 0);
  const past = planeFades(WIRING_BAND[1]);
  assert.equal(past.panels, 0);
  assert.equal(past.board, 1);
  assert.equal(past.openWiring, 1);
  assert.equal(past.sheet, 0);
  // Monotonic up to the control band: setup and panels never rise, the Wiring never falls.
  let prev = planeFades(0.01);
  for (let r = 0.01; r <= CONTROL_BAND[0]; r += 0.005) {
    const f = planeFades(r);
    assert.ok(f.setup <= prev.setup + 1e-12 && f.panels <= prev.panels + 1e-12 && f.board >= prev.board - 1e-12, `at ${r}`);
    close(f.panels + f.board, 1);
    prev = f;
  }
  // No timer: the same ratio is the same picture, zooming in or out.
  assert.deepEqual(planeFades(1.7), planeFades(1.7));
});

test('phase 2: each new band is monotonic across it, exactly 0 and 1 at its ends; the opened instrument cross-fades, the others go to SURROUND', () => {
  const engine = engineBand(8);
  const at = (r) => planeFades(r, engine);
  // The control band: the Control plane and the live controls in, the opened Wiring box out, the other boxes to SURROUND.
  assert.equal(at(CONTROL_BAND[0]).sheet, 0);
  assert.equal(at(CONTROL_BAND[1]).sheet, 1);
  assert.equal(at(CONTROL_BAND[0]).openWiring, 1);
  assert.equal(at(CONTROL_BAND[1]).openWiring, 0);
  close(at(CONTROL_BAND[1]).board, SURROUND);
  assert.equal(at(CONTROL_BAND[0]).controls, 0);
  assert.equal(at(CONTROL_BAND[1]).controls, 1);
  // The engine band: the board's Engine in, the Control plane around it to SURROUND.
  assert.equal(at(engine[0]).deep, 0);
  assert.equal(at(engine[1]).deep, 1);
  assert.equal(at(engine[0]).sheet, 1);
  close(at(engine[1]).sheet, SURROUND);
  const bands = [
    [WIRING_BAND, { board: +1, openWiring: +1, panels: -1, controls: -1 }],
    [CONTROL_BAND, { sheet: +1, controls: +1, openWiring: -1, board: -1 }],
    [engine, { deep: +1, sheet: -1, controls: -1 }],
  ];
  for (const [band, keys] of bands) {
    let prev = at(band[0]);
    for (let i = 1; i <= 400; i++) {
      const f = at(band[0] + ((band[1] - band[0]) * i) / 400);
      for (const [key, sign] of Object.entries(keys)) assert.ok(sign * (f[key] - prev[key]) >= -1e-12, `${key} monotonic in [${band}] at step ${i}`);
      prev = f;
    }
  }
  // The bands nest: none overlaps the next.
  assert.ok(WIRING_BAND[1] <= CONTROL_BAND[0] && CONTROL_BAND[1] <= engine[0] && engine[0] < engine[1]);
  for (const cubeR of [0, 1, 3, 4, 6, 40, Number.NaN]) {
    const b = engineBand(cubeR);
    assert.ok(b[0] >= CONTROL_BAND[1] + ENGINE_GAP - 1e-12 && b[1] - b[0] >= ENGINE_GAP - 1e-12, `engine band of ${cubeR}`);
  }
  assert.equal(engineBand(40)[1], 40);
});

test('the level follows the ratio to the fit', () => {
  assert.equal(levelOf(0.5), 'setup');
  assert.equal(levelOf(0.9999), 'setup');
  assert.equal(levelOf(1), 'stack');
  assert.equal(levelOf(1 - 1e-9), 'stack');
  assert.equal(levelOf(1.04), 'stack');
  assert.equal(levelOf(WIRING_BAND[1]), 'wiring');
  assert.equal(levelOf(CONTROL_BAND[0] + 0.1), 'wiring');
  assert.equal(levelOf(CONTROL_BAND[1]), 'control');
  assert.equal(levelOf(ENGINE_BAND[1]), 'engine');
  assert.equal(levelOf(5, engineBand(8)), 'control');
  assert.equal(levelOf(8, engineBand(8)), 'engine');
  assert.ok(opensAt(WIRING_BAND[1]) && !opensAt(CONTROL_BAND[0]) && !opensAt(CONTROL_BAND[1]));
  assert.ok(reachesFit(0.9, 1.1));
  assert.ok(!reachesFit(1, 1.2));
  assert.ok(!reachesFit(0.5, 0.8));
});

test('the instrument that opens is the one under the zoom point, else the nearest', () => {
  const rects = [
    { entry: 'card:a', x: 0, y: 0, w: 100, h: 50 },
    { entry: 'card:b', x: 0, y: 66, w: 100, h: 80 },
  ];
  assert.equal(instrumentAt(rects, { x: 50, y: 20 }), 'card:a');
  assert.equal(instrumentAt(rects, { x: 50, y: 100 }), 'card:b');
  assert.equal(instrumentAt(rects, { x: 50, y: 400 }), 'card:b');
  assert.equal(instrumentAt(rects, { x: 50, y: 57 }), 'card:a');
  assert.equal(instrumentAt([], { x: 0, y: 0 }), null);
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
  // The limit is a depth: its scale is the depth less the lift.
  close(capped.k, MAX_R - LIFT);
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

test('the Engine’s view lays the board’s frame on the cube, centred, and maps back', () => {
  const frame = { x: 200, y: -40, w: 600, h: 300 };
  const view = { k: 6, x: -900, y: -2000 };
  const cube = { x: 400, y: 500, w: 144, h: 96 };
  const offset = { x: 0, y: 0 };
  const onto = screenRect(view, cube);
  const board = frameOnto(frame, onto, offset);
  // The frame fits the cube's rectangle on screen, centred on its centre (the zoom point stays put).
  const fx = (frame.x + frame.w / 2) * board.k + board.x;
  const fy = (frame.y + frame.h / 2) * board.k + board.y;
  close(fx, onto.x + onto.w / 2);
  close(fy, onto.y + onto.h / 2);
  assert.ok(frame.w * board.k <= onto.w + 1e-9 && frame.h * board.k <= onto.h + 1e-9);
  const back = planeFromFrame(board, frame, cube, offset);
  close(back.k, view.k, 1e-9);
  close(back.x, view.x, 1e-6);
  close(back.y, view.y, 1e-6);
  // Zooming the plane zooms the board in proportion: one curve through the hand-off.
  const twice = frameOnto(frame, screenRect({ k: 12, x: view.x * 2, y: view.y * 2 }, cube), offset);
  close(twice.k, board.k * 2, 1e-9);
});

test('the fades are continuous across every band: no step anywhere, the steepest change the smoothstep’s', () => {
  // A smoothstep over [a, b] changes by at most 1.5 / (b − a) per unit of r; sampled finely, no step exceeds that slope.
  const engine = engineBand(5);
  const slope = Math.max(...[FOCUS_BAND, WIRING_BAND, CONTROL_BAND, engine].map((b) => 1.5 / (b[1] - b[0])));
  const dr = 1e-4;
  let prev = planeFades(MIN_R, engine);
  for (let r = MIN_R + dr; r <= engine[1] + 0.5; r += dr) {
    const f = planeFades(r, engine);
    for (const key of ['setup', 'panels', 'board', 'openWiring', 'sheet', 'controls', 'deep']) assert.ok(Math.abs(f[key] - prev[key]) <= 2 * slope * dr * 1.0001 + 1e-12, `${key} steps at r ${r}`);
    prev = f;
  }
  // The band ends meet their neighbours exactly: the level's change at the fit and at the band's end shows nothing new.
  assert.equal(planeFades(FOCUS_BAND[0]).setup, 1);
  assert.equal(planeFades(1 - 1e-12).setup, planeFades(1).setup);
  assert.equal(planeFades(WIRING_BAND[0]).board, 0);
  assert.equal(planeFades(WIRING_BAND[1] - 1e-12).panels, planeFades(WIRING_BAND[1]).panels);
  assert.equal(planeFades(WIRING_BAND[1]).panels, 0);
  assert.ok(MAX_R >= ENGINE_BAND[1]);
});

test('zoom never changes the selection: through every band and back, only the view and the instrument that opens change', () => {
  const viewport = { w: 1400, h: 900 };
  const stack = { x: 0, y: 0, w: 1280, h: 1500 };
  const fit = fitScale(stack, viewport);
  const rects = [
    { entry: 'card:transport', x: 0, y: 80, w: 1280, h: 120 },
    { entry: 'card:mono-synth', x: 0, y: 216, w: 1280, h: 420 },
    { entry: 'card:rhythm-composer', x: 0, y: 652, w: 1280, h: 400 },
  ];
  const selection = Object.freeze({ kind: 'cable', from: '/synth.out', to: '/echo.in' });
  const extra = Object.freeze({ note: 'kept' });
  let state = { view: fitWidth(stack, viewport), opened: 'card:transport', selection, extra };
  const engine = engineBand(6);
  // In, about a point over MONO SYNTH: it opens below the control band, then stays put.
  const at = toScreen(state.view, { x: 600, y: 400 });
  const seen = [];
  for (let i = 0; i < 80; i++) {
    const before = state;
    state = zoomStep(state, 1.05, at, { fit, rects, maxR: engine[1] });
    assert.equal(state.selection, selection, 'the selection is the very same object');
    assert.equal(state.extra, extra);
    if (!opensAt(depthOf(before.view.k / fit, before.lift))) assert.equal(state.opened, before.opened, 'inside the control band the opened instrument stays');
    seen.push({ z: depthOf(state.view.k / fit, state.lift), k: state.view.k });
  }
  assert.equal(state.opened, 'card:mono-synth');
  assert.ok(Math.abs(depthOf(state.view.k / fit, state.lift) - engine[1]) < 1e-9, 'the zoom stops at the engine band’s end');
  // Through the wiring band the scale stays the fit: the whole Stack's width stays on stage.
  for (const { z, k } of seen) if (z >= 1 && z <= 1 + LIFT) assert.ok(Math.abs(k - fit) < 1e-9, `at depth ${z} the scale is the fit`);
  // And out again: the selection never moves, whatever the zoom.
  for (let i = 0; i < 80; i++) {
    state = zoomStep(state, 1 / 1.05, at, { fit, rects, maxR: engine[1] });
    assert.equal(state.selection, selection);
  }
  // The zoom point stays put through the bands (the nested footprints: nothing slides).
  const p = toPlane(state.view, at);
  assert.ok(Math.abs(p.x - 600) < 1e-6 && Math.abs(p.y - 400) < 1e-6);
  assert.ok(seen.some(({ z }) => z > CONTROL_BAND[1]) && seen.some(({ z }) => z > WIRING_BAND[0] && z < WIRING_BAND[1]));
  assert.equal(state.lift, 0, 'zoomed back out of the fit: no lift left');
});

test('the depth: the lift at the fit, then the scale; continuous and monotonic both ways', () => {
  assert.deepEqual(splitDepth(0.5), { r: 0.5, lift: 0 });
  assert.deepEqual(splitDepth(1.2), { r: 1, lift: 0.19999999999999996 });
  close(splitDepth(2).r, 2 - LIFT);
  let prev = -Infinity;
  for (let z = 0.5; z < 4; z += 0.001) {
    const { r, lift } = splitDepth(z);
    const back = depthOf(r, lift);
    close(back, z, 1e-9);
    assert.ok(back > prev);
    prev = back;
  }
  assert.equal(depthOf(0.8, LIFT), 0.8, 'no lift below the fit');
});
