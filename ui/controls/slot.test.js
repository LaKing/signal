// ADR 0046 §5 (the operator's visual check, 2026-10-30): the open slot card
// kept the part it read on `this.part`, which on an element is the DOM's own
// `part` (its `::part()` names): the assignment wrote `part="[object Object]"`
// and the card never saw a part. Driven with the least of a DOM, whose
// HTMLElement has `part` as a browser has it; and no control assigns an
// element's own `part` or `slot`.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';

class Element {
  constructor() {
    this.attributesMap = new Map();
  }
  attachShadow() {
    this.shadowRoot = { querySelector: () => null, append() {}, firstChild: null };
    return this.shadowRoot;
  }
  getAttribute(name) {
    return this.attributesMap.get(name) ?? null;
  }
  setAttribute(name, value) {
    this.attributesMap.set(name, String(value));
  }
  hasAttribute(name) {
    return this.attributesMap.has(name);
  }
  addEventListener() {}
}
// As in a browser: `part` reflects the attribute (a DOMTokenList, [PutForwards=value]).
Object.defineProperty(Element.prototype, 'part', {
  get() {
    return { value: this.getAttribute('part') ?? '' };
  },
  set(value) {
    this.setAttribute('part', value);
  },
});
globalThis.HTMLElement = Element;
globalThis.customElements ??= { get: () => undefined, define: () => {} };

const { SignalSlot } = await import('./slot.js');

test("the slot card keeps the part it read as its own state, never in the element's `part`", async () => {
  const slot = new SignalSlot();
  // The element's own attributes and `part`, as a browser has them (whichever stub the other tests installed first).
  const attributes = new Map();
  Object.assign(slot, { getAttribute: (n) => attributes.get(n) ?? null, setAttribute: (n, v) => attributes.set(n, String(v)) });
  Object.defineProperty(slot, 'part', { get: () => ({ value: attributes.get('part') ?? '' }), set: (v) => attributes.set('part', String(v)) });
  slot.setAttribute('bind', '/synth');
  let shown = 0;
  slot.showPart = async () => {
    shown += 1;
  };
  slot.recheck = () => {};
  slot.partWritten({ type: 'panel.written', file: 'parts/synth.html', base: 'x', html: '\n  <div class="signal-cards"></div>\n' });
  await Promise.resolve();
  assert.equal(slot.getAttribute('part'), null, 'no part attribute written');
  assert.equal(slot.partState.exists, true);
  assert.equal(shown, 1);
  slot.partWritten({ type: 'panel.written', file: 'parts/other.html', html: '' });
  assert.equal(shown, 1, "another file's write is not this slot's");
});

test("no control assigns an element's own `part` or `slot`", () => {
  const dir = new URL('./', import.meta.url);
  for (const file of readdirSync(dir).filter((f) => f.endsWith('.js') && !f.endsWith('.test.js'))) {
    const source = readFileSync(new URL(file, dir), 'utf8');
    assert.doesNotMatch(source, /\bthis\.(part|slot)\s*=[^=]/, file);
  }
});
