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

// The operator's second check (2026-10-30): the card was attached while its
// connection was connecting, its read refused (`closed`) and shown as the
// part's error, the generic knobs drawn. A read refused so is made again
// once the connection opens; any other failure is said, never hidden.
test('the part is read once the connection opens; a failure is said, not hidden by the knobs', async () => {
  const listeners = new Map();
  const connection = {
    status: 'connecting',
    description: null,
    on: (event, fn) => {
      listeners.set(event, fn);
      return () => {};
    },
    request: async (message) => {
      if (connection.status !== 'open') throw Object.assign(new Error('connection closed'), { code: 'closed' });
      if (connection.fail) throw Object.assign(new Error('the disk is gone'), { code: 'unknown' });
      return { type: 'panel.read', file: message.file, exists: true, editable: true, html: '\n  <div class="signal-cards"></div>\n' };
    },
  };
  const slot = new SignalSlot();
  const attributes = new Map([['bind', '/echo']]);
  Object.assign(slot, { getAttribute: (n) => attributes.get(n) ?? null, setAttribute: (n, v) => attributes.set(n, String(v)) });
  slot.showPart = async () => {};
  slot.attach(connection);
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(slot.partState, null, 'not read while connecting');
  connection.status = 'open';
  listeners.get('status')('open');
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(slot.partState.exists, true, 'read once open');
  connection.fail = true;
  slot.partState = null;
  await slot.readPart(connection);
  assert.match(slot.partState.error, /the disk is gone/);
});
