import test from 'node:test';
import assert from 'node:assert/strict';

import { formatValue } from './format.js';

test('values are formatted with their unit', () => {
  assert.equal(formatValue(440, 'Hz'), '440.0 Hz');
  assert.equal(formatValue(12345, 'Hz'), '12.35 kHz');
  assert.equal(formatValue(-6.02, 'dB'), '-6.0 dB');
  assert.equal(formatValue(0.25, 's'), '250.0 ms');
  assert.equal(formatValue(1500, 'ms'), '1.50 s');
  assert.equal(formatValue(0.5), '0.500');
  assert.equal(formatValue(42.123, '%'), '42.12 %');
  assert.equal(formatValue(0.5, null, { digits: 1 }), '0.5');
  assert.equal(formatValue(-Infinity, 'dB'), '−∞ dB');
  assert.equal(formatValue(NaN, 'Hz'), '—');
});
