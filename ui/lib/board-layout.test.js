// ADR 0047 §5–6 (phase 2): the layered layout and the anchored placement (ui/lib/board-layout.js).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { CLEARANCE, PU, SPIRAL_RINGS, balance, centroid, inversions, isDummy, near, placeAnchored, ring, sugiyama } from './board-layout.js';

test('inversions are counted as a merge sort counts them', () => {
  assert.equal(inversions([]), 0);
  assert.equal(inversions([1, 2, 3]), 0);
  assert.equal(inversions([3, 2, 1]), 3);
  assert.equal(inversions([2, 4, 1, 3, 5]), 3);
  const values = [5, 1, 4, 2, 2, 3, 0];
  let slow = 0;
  for (let i = 0; i < values.length; i++) for (let j = i + 1; j < values.length; j++) if (values[i] > values[j]) slow++;
  assert.equal(inversions(values), slow);
});

test('the balancing pass pulls a source right, next to its successors; kept nodes stay', () => {
  const ids = ['clock', 'a', 'b', 'c'];
  const edges = [
    { from: 'a', to: 'b' },
    { from: 'b', to: 'c' },
    { from: 'clock', to: 'c' },
  ];
  const layer = new Map([['clock', 0], ['a', 0], ['b', 1], ['c', 2]]);
  const balanced = balance(ids, edges, layer);
  assert.equal(balanced.get('clock'), 1, 'just before c');
  assert.equal(balanced.get('a'), 0, 'its successor is next already');
  assert.equal(balance(ids, edges, layer, (id) => id === 'clock').get('clock'), 0);
});

/** A small level: two sources, each feeding the other's sink, so the first order crosses. */
function crossed() {
  const nodes = ['a', 'b', 'x', 'y'].map((id) => ({ id, w: 100, h: 60 }));
  const edges = [
    { key: 0, from: 'a', to: 'y', net: 'a', fromOff: 42, toOff: 42 },
    { key: 1, from: 'b', to: 'x', net: 'b', fromOff: 42, toOff: 42 },
  ];
  const layer = new Map([['a', 0], ['b', 0], ['x', 1], ['y', 1]]);
  return { nodes, edges, layer };
}

test('crossing minimisation: the sweeps undo a crossing the source order makes', () => {
  const { nodes, edges, layer } = crossed();
  const lay = sugiyama(nodes, edges, layer);
  assert.equal(lay.crossings, 0);
  const order = lay.rows.map((row) => row.join(' '));
  assert.ok(order[1] === 'y x' || order[0] === 'b a', `the second layer follows the first: ${order}`);
});

test('a long edge gets a dummy per layer it crosses, its wire straight through them', () => {
  const nodes = ['a', 'b', 'c', 'd'].map((id) => ({ id, w: 80, h: 58 }));
  const edges = [
    { key: 0, from: 'a', to: 'b', net: 'a', fromOff: 42, toOff: 42 },
    { key: 1, from: 'b', to: 'c', net: 'b', fromOff: 42, toOff: 42 },
    { key: 2, from: 'c', to: 'd', net: 'c', fromOff: 42, toOff: 42 },
    { key: 3, from: 'a', to: 'd', net: 'a', fromOff: 42, toOff: 42 },
  ];
  const layer = new Map([['a', 0], ['b', 1], ['c', 2], ['d', 3]]);
  const lay = sugiyama(nodes, edges, layer);
  const chain = lay.chains.get(3);
  assert.equal(chain.length, 2);
  assert.deepEqual(chain.map((d) => d.layer), [1, 2]);
  assert.equal(chain[0].y, chain[1].y, 'straight through the columns it crosses');
  assert.ok(lay.rows.flat().filter(isDummy).length === 2);
  // Columns left to right, each channel wide enough for its tracks.
  for (let i = 1; i < lay.columns.length; i++) assert.ok(lay.columns[i].x >= lay.columns[i - 1].x + lay.columns[i - 1].w + 72);
  // A box and a dummy of one column never overlap.
  for (const [i, row] of lay.rows.entries()) {
    for (const id of row.filter((r) => !isDummy(r))) {
      const top = lay.y.get(id);
      for (const d of [...lay.chains.values()].flat().filter((d) => d.layer === i)) assert.ok(d.y < top - 6 || d.y > top + 58 + 6, `${id} and a dummy in column ${i}`);
    }
  }
});

test('the layout is a function of its input: the same twice, whole numbers', () => {
  const { nodes, edges, layer } = crossed();
  const a = sugiyama(nodes, edges, layer);
  const b = sugiyama(nodes, edges, layer);
  assert.deepEqual([...a.y], [...b.y]);
  assert.deepEqual([...a.x], [...b.x]);
  for (const v of a.y.values()) assert.equal(v, Math.round(v));
});

test('the spiral visits each ring nearest first, in a fixed order', () => {
  assert.deepEqual(ring(0), [[0, 0]]);
  assert.equal(ring(1).length, 8);
  assert.equal(ring(3).length, 24);
  assert.deepEqual(ring(1).slice(0, 4), [[0, -1], [-1, 0], [1, 0], [0, 1]]);
  assert.deepEqual(ring(2), ring(2), 'the same each time');
});

test('an anchored chip lies near its vias’ centroid, covering no via, control or chip (test 7)', () => {
  const vias = [
    { x: 80, y: 80, w: 64, h: 96 },
    { x: 160, y: 80, w: 64, h: 96 },
    { x: 240, y: 80, w: 64, h: 96 },
  ];
  const others = [{ x: 400, y: 80, w: 64, h: 96 }, { x: 80, y: 260, w: 240, h: 120 }];
  const chip = { w: 120, h: 90 };
  const at = placeAnchored(chip, vias, [...vias, ...others]);
  assert.ok(at);
  assert.equal(Math.abs(at.x % PU), 0);
  assert.equal(Math.abs(at.y % PU), 0);
  const box = { ...at, ...chip };
  for (const o of [...vias, ...others]) assert.ok(!near(box, o, CLEARANCE), `clear of ${JSON.stringify(o)}`);
  const c = centroid(vias);
  const d = Math.hypot(at.x + chip.w / 2 - c.x, at.y + chip.h / 2 - c.y);
  assert.ok(d <= SPIRAL_RINGS * PU * Math.SQRT2, `within the spiral: ${d}`);
  assert.ok(d < 200, `near: ${d}`);
  // Nothing in the way: centred on them, snapped.
  assert.deepEqual(placeAnchored(chip, vias, []), { x: Math.round((c.x - 60) / PU) * PU, y: Math.round((c.y - 45) / PU) * PU });
  assert.equal(placeAnchored(chip, [], []), null);
});
