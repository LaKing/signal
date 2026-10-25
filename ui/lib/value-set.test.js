import test from 'node:test';
import assert from 'node:assert/strict';
import { isDiscrete, nearestValue, selectOptions, selectValue, valueLabel } from './value-set.js';
import { rows, structureKey } from '../app/lib/parameters.js';

const SHAPES = [
  { value: 0, label: 'sine' },
  { value: 1, label: 'saw' },
  { value: 2, label: 'square' },
  { value: 3, label: 'triangle' },
];

test('the nearest value: the greater of two as near (as the modules read it); the value itself without a set', () => {
  assert.equal(nearestValue(SHAPES, 1.4), 1);
  assert.equal(nearestValue(SHAPES, 0.5), 1, 'saw, as std.osc plays 0.5');
  assert.equal(nearestValue(SHAPES, 1.5), 2);
  assert.equal(nearestValue(SHAPES, 0.49999999999999994), 1, 'as ⌊x + ½⌋ reads it');
  assert.equal(nearestValue([{ value: 0 }, { value: 0.25 }, { value: 1 }], 0.125), 0.25, 'elsewhere a tie upward');
  assert.equal(nearestValue(SHAPES, 1.6), 2);
  assert.equal(nearestValue(SHAPES, -9), 0);
  assert.equal(nearestValue(SHAPES, 99), 3);
  assert.equal(nearestValue(null, 0.25), 0.25);
});

test('a value set read as positive: anything above 0 is on, as std.to_events reads on_change', () => {
  const OFF_ON = [{ value: 0, label: 'off' }, { value: 1, label: 'on' }];
  assert.equal(nearestValue(OFF_ON, 0.3, 'positive'), 1);
  assert.equal(nearestValue(OFF_ON, 1e-9, 'positive'), 1);
  assert.equal(nearestValue(OFF_ON, 0, 'positive'), 0);
  assert.equal(nearestValue(OFF_ON, 0.3), 0, 'the nearest without it');
  assert.equal(selectValue(OFF_ON, 0.4, 'positive'), '1');
  assert.equal(valueLabel(OFF_ON, 0.4, 'positive'), 'on (0.4)');
});

test('a selector: one option per value, named; it shows the nearest value', () => {
  assert.deepEqual(selectOptions(SHAPES).map((o) => [o.value, o.label]), [['0', 'sine'], ['1', 'saw'], ['2', 'square'], ['3', 'triangle']]);
  assert.equal(selectValue(SHAPES, 2), '2');
  assert.equal(selectValue(SHAPES, 2.2), '2');
  assert.equal(selectValue(SHAPES, NaN), '');
  assert.equal(selectValue(null, 1), '');
});

test('a value reads as its name, or the nearest name with the number', () => {
  assert.equal(valueLabel(SHAPES, 1), 'saw');
  assert.equal(valueLabel(SHAPES, 1.002265625), 'saw (1.002)');
  assert.equal(valueLabel(SHAPES, NaN), '');
  assert.equal(isDiscrete({ values: SHAPES }), true);
  assert.equal(isDiscrete({ values: null }), false);
  assert.equal(isDiscrete({}), false);
});

test('the Parameters tab: a control’s and an inner parameter’s value set reach the rows and the structure', () => {
  const description = {
    controls: [
      { address: '/synth.osc1_wave', instance: 'synth', parameter: 'osc1_wave', module: 'moog_voice', min: 0, max: 3, default: 1, base: 2, inFile: true, values: SHAPES },
      { address: '/synth.volume', instance: 'synth', parameter: 'volume', module: 'moog_voice', unit: 'dB', min: -120, max: 12, default: 0, base: 0, inFile: false },
    ],
    instances: [{ address: '/synth/osc1', parameters: [{ name: 'shape', address: '/synth/osc1.shape', range: { min: 0, max: 3 }, default: 1, base: 1, values: SHAPES }] }],
  };
  const [synth] = rows(description);
  assert.deepEqual(synth.parameters[0].values, SHAPES);
  assert.equal(synth.parameters[1].values, null, 'continuous');
  assert.deepEqual(synth.inner[0].values, SHAPES);
  const without = rows({ ...description, controls: description.controls.map((c) => ({ ...c, values: undefined })) });
  assert.notEqual(structureKey([synth]), structureKey(without), 'a value set changes the structure (a selector, not a slider)');
});
