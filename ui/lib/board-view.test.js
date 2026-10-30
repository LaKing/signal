// ADR 0047 §5 (phase 2): the device view's state and the plate's measures (ui/lib/board-view.js).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DEFAULT_VIEW, LAYERS, metricsText, nextLayer, plateSize, plateVias, shownWires, toggleKind, viaRect, viewState } from './board-view.js';

test('the view state: anything unknown is the default’s; kinds in their order', () => {
  assert.deepEqual(viewState(null), { layer: 'board', hidden: [] });
  // One arrangement (the owner's verdict): an old state's `arrange` is dropped.
  assert.deepEqual(viewState({ layer: 'xray', arrange: 'flow', hidden: ['vector', 'midi', 'signal'] }), { layer: 'xray', hidden: ['signal', 'vector'] });
  assert.deepEqual(viewState({ layer: 'top', hidden: 'signal' }), { ...DEFAULT_VIEW, hidden: [] });
});

test('one key steps through Plate, X-ray, Board and round again', () => {
  assert.deepEqual(LAYERS, ['plate', 'xray', 'board']);
  assert.equal(nextLayer('plate'), 'xray');
  assert.equal(nextLayer('xray'), 'board');
  assert.equal(nextLayer('board'), 'plate');
  assert.equal(nextLayer('nonsense'), 'plate');
});

test('a kind toggles on and off', () => {
  assert.deepEqual(toggleKind([], 'events'), ['events']);
  assert.deepEqual(toggleKind(['vector', 'events'], 'events'), ['vector']);
  assert.deepEqual(toggleKind(['vector'], 'signal'), ['signal', 'vector']);
});

test('vias in whole plate units: a fraction of a pixel never moves the board (§5)', () => {
  assert.deepEqual(viaRect({ x: 19.6, y: 4.2, w: 71.5, h: 2 }), { x: 16, y: 8, w: 72, h: 8 });
  // WebKit and Chrome a fraction apart: the same via.
  assert.deepEqual(viaRect({ x: 100.31, y: 40.2, w: 63.9, h: 96.4 }), viaRect({ x: 100.29, y: 39.9, w: 64.1, h: 95.7 }));
  const controls = [{ index: 0 }, { index: 1 }, { index: 2 }];
  const measured = new Map([[0, { x: 10, y: 10, w: 60, h: 90 }], [2, { x: 100, y: 10, w: 60, h: 90 }]]);
  const { vias, obstacles, key } = plateVias(controls, (c) => measured.get(c.index) ?? null, [{ x: 200, y: 0, w: 30, h: 30 }]);
  assert.deepEqual([...vias.keys()], [0, 2], 'a control not drawn has no via');
  assert.equal(obstacles.length, 3, 'every control drawn is an obstacle');
  assert.equal(key, plateVias(controls, (c) => measured.get(c.index) ?? null, [{ x: 200.2, y: 0, w: 30, h: 30 }]).key);
  assert.deepEqual(plateSize(1280, 601), { w: 1280, h: 608 });
});

test('the measures as the bar says them, never a verdict', () => {
  assert.equal(metricsText({ crossings: 3, bends: 40, length: 10324, labels: 1 }), '3 crossings · 40 bends · length 10.3k · 1 labelled nets');
  assert.equal(metricsText(null), '');
});

test('the wires drawn: a hidden kind’s left out, a labelled net traced drawn whole', () => {
  const whole = { id: 1, shape: 'signal', whole: true };
  const wires = [{ id: 0, shape: 'events' }, { id: 1, shape: 'signal', labelled: true, whole }, { id: 2, shape: 'signal', labelled: true, whole: { id: 2 } }];
  const shown = shownWires(wires, ['events'], new Set([1]), (w) => w.id);
  assert.deepEqual(shown.map((w) => w.id), [1, 2]);
  assert.equal(shown[0], whole);
  assert.equal(shown[1], wires[2], 'not traced: its labels');
});
