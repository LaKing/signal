import test from 'node:test';
import assert from 'node:assert/strict';

import { bipolarOrigin, effectiveTaper, toPosition, toValue } from './taper.js';

const RANGES = {
  linear: { min: -1, max: 1, taper: 'linear' },
  log: { min: 20, max: 20000, taper: 'log' },
  db: { min: 0, max: 4, taper: 'db' },
};

const close = (a, b, tolerance, what) => assert.ok(Math.abs(a - b) <= tolerance, `${what}: ${a} vs ${b}`);

test('position → value → position round-trips at every taper', () => {
  for (const [name, range] of Object.entries(RANGES)) {
    for (let i = 0; i <= 100; i++) {
      const p = i / 100;
      close(toPosition(toValue(p, range), range), p, 1e-9, `${name} at ${p}`);
    }
  }
});

test('value → position → value round-trips at every taper, above the dB floor', () => {
  const values = {
    linear: [-1, -0.37, 0, 0.5, 1],
    log: [20, 21.5, 440, 1000, 19999, 20000],
    // The dB taper's domain: values above the floor's value (−60 dB: 0.001) and min itself.
    db: [0, 0.0011, 0.01, 0.5, 1, 2, 4],
  };
  for (const [name, range] of Object.entries(RANGES)) {
    for (const v of values[name]) {
      const back = toValue(toPosition(v, range), range);
      close(back, v, 1e-9 * Math.max(1, Math.abs(v)), `${name} at ${v}`);
    }
  }
});

test('the dB taper clamps values at or below its floor to position 0 and min', () => {
  const range = RANGES.db;
  for (const v of [0.0001, 0.001]) {
    assert.equal(toPosition(v, range), 0);
    assert.equal(toValue(toPosition(v, range), range), 0);
  }
  // 0 dB on a 0..4 gain sits where it should: (0 − (−60)) / (12.04 − (−60)).
  close(toPosition(1, range), 60 / (60 + 20 * Math.log10(4)), 1e-12, '0 dB');
  // A positive min raises the floor to min in dB, so position 0 is continuous.
  const raised = { min: 0.1, max: 1, taper: 'db' };
  close(toPosition(0.1, raised), 0, 1e-12, 'min');
  close(toValue(0.5, raised), 10 ** (-10 / 20), 1e-12, 'midway in dB');
});

test('endpoints, the log fallback and clamping', () => {
  for (const range of Object.values(RANGES)) {
    assert.equal(toValue(0, range), range.min);
    assert.equal(toValue(1, range), range.max);
    assert.equal(toPosition(range.max * 10, range), 1);
    assert.equal(toValue(2, range), range.max);
  }
  close(toValue(0.5, RANGES.log), Math.sqrt(20 * 20000), 1e-9, 'geometric middle');
  assert.equal(effectiveTaper({ min: 0, max: 1, taper: 'log' }), 'linear');
  assert.equal(toValue(0.25, { min: 0, max: 1, taper: 'log' }), 0.25);
  assert.equal(effectiveTaper({ min: -1, max: 1, taper: 'db' }), 'linear');
  assert.equal(effectiveTaper({ min: 0, max: 1, taper: 'nonsense' }), 'linear');
});

test('a bipolar arc starts at zero, or the middle', () => {
  assert.equal(bipolarOrigin({ min: -1, max: 1 }), 0.5);
  close(bipolarOrigin({ min: -1, max: 3 }), 0.25, 1e-12, 'zero');
  assert.equal(bipolarOrigin({ min: 1, max: 3 }), 0.5);
});
