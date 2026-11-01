// ADR 0051 phase 2 (ADR 0047 phase 4: every drawn element is hit): the
// Wiring and Control drawings' hit-testing, pins before wires before
// boxes, at a reach fixed on screen; every port, pin and cable of the
// studio's Wiring is hit at its own place; culling keeps what is on the
// stage.

import test from 'node:test';
import assert from 'node:assert/strict';
import { PIN_PX, WIRE_PX, hitItem, itemBox, polylineDistance, visibleIds } from './plane-hit.js';

const items = [
  { kind: 'box', id: 'box a', x: 0, y: 0, w: 200, h: 100 },
  { kind: 'cable', id: 'cable 0', points: [{ x: 200, y: 50 }, { x: 260, y: 50 }, { x: 260, y: 150 }] },
  { kind: 'port', id: 'port out /a.out', x: 200, y: 50 },
];

test('pins win over wires, wires over boxes; the reach is fixed on screen', () => {
  assert.equal(hitItem(items, { x: 201, y: 51 }, 1).item.id, 'port out /a.out');
  assert.equal(hitItem(items, { x: 240, y: 52 }, 1).item.id, 'cable 0');
  assert.equal(hitItem(items, { x: 100, y: 50 }, 1).item.id, 'box a');
  assert.equal(hitItem(items, { x: 400, y: 400 }, 1), null);
  // Zoomed out 4×, a wire 12 plane units away is 3 px on screen: hit; zoomed in 4×, 48 px: not.
  assert.equal(hitItem(items, { x: 240, y: 62 }, 0.25)?.item.id, 'cable 0');
  assert.equal(hitItem(items, { x: 240, y: 62 }, 4), null);
  assert.ok(PIN_PX > WIRE_PX);
});

test('distances and boxes', () => {
  assert.equal(polylineDistance({ x: 230, y: 40 }, items[1].points), 10);
  assert.deepEqual(itemBox(items[1]), { x: 200, y: 50, w: 60, h: 100 });
  assert.deepEqual(itemBox(items[2]), { x: 200, y: 50, w: 0, h: 0 });
});

test('culling: only what meets the stage (with its margin) is drawn', () => {
  assert.deepEqual([...visibleIds(items, { x: 300, y: 0, w: 100, h: 100 })], []);
  assert.deepEqual([...visibleIds(items, { x: 300, y: 0, w: 100, h: 100 }, 40)], ['cable 0']);
  assert.deepEqual([...visibleIds(items, { x: 0, y: 0, w: 1000, h: 1000 })], ['box a', 'cable 0', 'port out /a.out']);
});
