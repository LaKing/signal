import test from 'node:test';
import assert from 'node:assert/strict';

import { alignXY, findTrigger, triggerStart } from './trigger.js';

// A sine of period 64 points, half a point late so no point sits on a
// crossing: sin(2π (i + ½) / 64). It crosses 0 rising between points 63 and
// 64, 0 falling between 31 and 32; 0.5 rising (θ = π/6, i ≈ 4.83) at 5, and
// 0.5 falling (θ = 5π/6, i ≈ 26.17) at 27.
const SINE = Float32Array.from({ length: 256 }, (_, i) => Math.sin((2 * Math.PI * (i + 0.5)) / 64));

test('the trigger finds the crossings of a known sine', () => {
  assert.equal(findTrigger(SINE, { level: 0, edge: 'rising' }), 64);
  assert.equal(findTrigger(SINE, { level: 0, edge: 'falling' }), 32);
  assert.equal(findTrigger(SINE, { level: 0.5, edge: 'rising' }), 5);
  assert.equal(findTrigger(SINE, { level: 0.5, edge: 'falling' }), 27);
  // The next period.
  assert.equal(findTrigger(SINE, { level: 0.5, edge: 'rising', from: 6 }), 69);
});

test('no crossing, no trigger: the display runs free', () => {
  assert.equal(findTrigger(SINE, { level: 1.5 }), -1);
  assert.equal(findTrigger(new Float32Array(10).fill(0.2), { level: 0 }), -1);
  assert.equal(triggerStart(SINE, 128, { level: 1.5 }), 0);
  assert.equal(triggerStart(SINE, 128, { level: 0, edge: 'falling' }), 32);
  assert.equal(triggerStart(SINE, 250, { level: 0, edge: 'falling' }), 0, 'too late to fill the display');
});

test('X–Y pairs the points two blocks share', () => {
  const x = { firstFrame: 1000, decimation: 10, values: Float32Array.from([0, 1, 2, 3, 4]) };
  const y = { firstFrame: 1020, decimation: 10, values: Float32Array.from([10, 11, 12, 13, 14]) };
  const pair = alignXY(x, y);
  assert.equal(pair.firstFrame, 1020);
  assert.deepEqual(Array.from(pair.xs), [2, 3, 4]);
  assert.deepEqual(Array.from(pair.ys), [10, 11, 12]);
  assert.deepEqual(Array.from(alignXY(y, x).xs), [10, 11, 12]);
  assert.equal(alignXY(x, { ...y, decimation: 5 }), null, 'different decimations');
  assert.equal(alignXY(x, { ...y, firstFrame: 1005 }), null, 'points on other frames');
  assert.equal(alignXY(x, { ...y, firstFrame: 2000 }), null, 'no overlap');
});
