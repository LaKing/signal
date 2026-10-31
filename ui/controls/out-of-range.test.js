// A control bound to a value outside its own range marks it: the
// `out-of-range` attribute and a tooltip, never over a title the page set
// (docs/ui-controls.md "Out of range").

import test from 'node:test';
import assert from 'node:assert/strict';

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };

const { markOutOfRange } = await import('./base.js');
const { Binding } = await import('../lib/binding.js');

/** An element with attributes, enough for a binding and the mark. */
function element(attributes = {}) {
  const a = new Map(Object.entries(attributes));
  return {
    getAttribute: (name) => (a.has(name) ? a.get(name) : null),
    hasAttribute: (name) => a.has(name),
    setAttribute: (name, value) => a.set(name, String(value)),
    removeAttribute: (name) => a.delete(name),
  };
}

function knob(base, attributes = {}) {
  const e = element({ min: '0', max: '2', ...attributes });
  const binding = new Binding(e, '/synth.key_track');
  binding.base = base;
  return { e, binding };
}

test('a value above the range: the attribute and the tooltip, gone once back in range', () => {
  const { e, binding } = knob(100000);
  assert.equal(markOutOfRange(e, [binding]), 'above');
  assert.equal(e.getAttribute('out-of-range'), 'above');
  assert.match(e.getAttribute('title'), /^Out of range: 100000\.0 is outside this control's range \(0\.000 … 2\.000\)/);
  binding.base = 1.5;
  assert.equal(markOutOfRange(e, [binding]), null);
  assert.equal(e.hasAttribute('out-of-range'), false);
  assert.equal(e.hasAttribute('title'), false);
});

test('only the effective value outside: `effective`', () => {
  const { e, binding } = knob(1);
  binding.effective = -1;
  assert.equal(markOutOfRange(e, [binding]), 'effective');
  assert.match(e.getAttribute('title'), /plays at/);
});

test('a title the page set is kept', () => {
  const { e, binding } = knob(-4, { title: 'mine' });
  assert.equal(markOutOfRange(e, [binding]), 'below');
  assert.equal(e.getAttribute('title'), 'mine');
  binding.base = 0;
  markOutOfRange(e, [binding]);
  assert.equal(e.getAttribute('title'), 'mine');
});
