import test from 'node:test';
import assert from 'node:assert/strict';

import { fixedWidth, formatFixed, formatValue } from './format.js';

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

// M2.8-11 (docs/ui-controls.md "Stable layout"): a readout never changes
// width while its value changes.
test('fixed-width values have the same length whatever the value', () => {
  const values = [0, 1e-9, -1e-9, 0.25, -0.25, 1, -6.02, 99.99, 100, 999.95, 1000, 12345, -12345, 99999.94, 123456.7, -1234567, 1e12, -1e12, 1e300, -1e300, NaN, Infinity, -Infinity];
  for (const unit of [null, 'dB', 'Hz', 's', 'ms', '%', 'Q']) {
    const width = fixedWidth(unit);
    for (const value of values) {
      const text = formatFixed(value, unit);
      assert.equal(text.length, width, `${value} ${unit}: "${text}"`);
    }
  }
  assert.equal(formatFixed(440, 'Hz'), '   440.0 Hz ');
  assert.equal(formatFixed(12345, 'Hz'), '   12.35 kHz');
  assert.equal(formatFixed(-6.02, 'dB'), '    -6.0 dB');
  assert.equal(formatFixed(1e12), ' 1.0e+12');
  assert.equal(formatFixed(-1e300), ' -1e+300');
});
