import test from 'node:test';
import assert from 'node:assert/strict';
import { applyProps, assignable } from './dom.js';

class Element {
  constructor() {
    this.attributes = {};
    this.textContent = '';
  }
  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }
}

// SignalMeter's `floor`: a getter without a setter.
class Meter extends Element {
  get floor() {
    return -60;
  }
  set label(value) {
    this.attributes.labelSet = value;
  }
}

test('a getter-only property gets the attribute instead of throwing (M2.8-11 AC1)', () => {
  const meter = new Meter();
  assert.throws(() => {
    'use strict';
    meter.floor = '-60';
  }, TypeError, 'what app.js did before');
  applyProps(meter, { floor: '-60', source: 'output', label: 'out 1', textContent: 'x' });
  assert.deepEqual(meter.attributes, { floor: '-60', source: 'output', labelSet: 'out 1' });
  assert.equal(meter.textContent, 'x');
});

test('only data properties and setters are assigned', () => {
  const meter = new Meter();
  assert.equal(assignable(meter, 'textContent'), true);
  assert.equal(assignable(meter, 'label'), true);
  assert.equal(assignable(meter, 'floor'), false);
  assert.equal(assignable(meter, 'channel'), false);
  assert.equal(assignable(Object.freeze({ a: 1 }), 'a'), false);
});
