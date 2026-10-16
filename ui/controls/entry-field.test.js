// Value entry in its owners (docs/ui-controls.md "Value entry", M2.9-4),
// driven with the least of a DOM they need: the field's lifetime, the
// Parameters tab's row, a control without a readout, and the 2D pad's
// readout. The look and the real events are checked by a person.

import test from 'node:test';
import assert from 'node:assert/strict';

class FakeElement {
  constructor(tag = 'div') {
    this.tagName = tag;
    this.listeners = {};
    this.children = [];
    this.parent = null;
    this.attributes = new Map();
    this.style = {};
    this.dataset = {};
    this.textContent = '';
    this.value = '';
    this.title = '';
  }
  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }
  getAttribute(name) {
    return this.attributes.get(name) ?? null;
  }
  hasAttribute(name) {
    return this.attributes.has(name);
  }
  addEventListener(type, listener) {
    (this.listeners[type] ??= []).push(listener);
  }
  dispatch(type, init = {}) {
    const event = { type, stopPropagation() {}, preventDefault() {}, ...init };
    for (const listener of this.listeners[type] ?? []) listener(event);
    return event;
  }
  append(...items) {
    for (const item of items) {
      if (typeof item !== 'object') continue;
      item.parent = this;
      this.children.push(item);
    }
  }
  remove() {
    if (this.parent) this.parent.children = this.parent.children.filter((c) => c !== this);
    this.parent = null;
  }
  focus() {
    document.activeElement = this;
  }
  select() {}
  closest() {
    return null;
  }
  getRootNode() {
    return document;
  }
  getBoundingClientRect() {
    return { left: 0, right: 0, top: 0, bottom: 0 };
  }
}

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };
globalThis.document ??= { createElement: (tag) => new FakeElement(tag), activeElement: null };

const { ValueEntry } = await import('./entry-field.js');
const { NOTE_MS } = await import('../lib/entry.js');

/** A field over a readout on 0..1 at 0.5, recording what it sends and says. */
function entryOn() {
  const readout = new FakeElement();
  const applied = [];
  const notes = [];
  const entry = new ValueEntry(readout, {
    spec: () => ({ unit: null, range: { min: 0, max: 1 } }),
    value: () => 0.5,
    apply: (value) => applied.push(value),
    note: (text) => notes.push(text),
  });
  const type = (text) => {
    entry.input.value = text;
    entry.input.dispatch('input');
  };
  return { readout, entry, applied, notes, type };
}

test('a double-click opens the field over the readout, with the value', () => {
  const { readout, entry } = entryOn();
  readout.dispatch('dblclick');
  assert.ok(entry.isOpen);
  assert.deepEqual(readout.children, [entry.input], 'the field is in the readout');
  assert.equal(entry.input.value, '0.5');
  assert.equal(document.activeElement, entry.input);
});

test('Escape after an edit sends nothing and closes the field', () => {
  const { readout, entry, applied, type } = entryOn();
  entry.open();
  type('0.9');
  entry.input.dispatch('keydown', { key: 'Escape' });
  assert.deepEqual(applied, []);
  assert.equal(entry.isOpen, false);
  assert.deepEqual(readout.children, []);
});

// auditor-1-004: a clamp committed by a click elsewhere is shown its time too.
test('a clamp committed by a click elsewhere keeps the field and the reason for NOTE_MS', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { readout, entry, applied, notes, type } = entryOn();
  entry.open();
  type('5');
  document.activeElement = null;
  entry.input.dispatch('blur');
  assert.deepEqual(applied, [1], 'sent clamped');
  assert.deepEqual(notes, ['Clamped to the maximum, 1.000']);
  assert.ok(entry.isOpen, 'the field stays');
  assert.equal(entry.input.value, '1', 'showing the clamped value');
  assert.equal(entry.input.readOnly, true);
  entry.input.dispatch('blur');
  assert.ok(entry.isOpen, 'a second blur does not close it');
  t.mock.timers.tick(NOTE_MS - 1);
  assert.ok(entry.isOpen);
  t.mock.timers.tick(1);
  assert.equal(entry.isOpen, false, 'closed when its time is up');
  assert.deepEqual(readout.children, []);
  assert.deepEqual(notes, ['Clamped to the maximum, 1.000', null], 'the reason cleared');
  assert.deepEqual(applied, [1], 'sent once');
});

test('a clamp committed by Enter is shown its time as well', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { entry, applied, notes, type } = entryOn();
  entry.open();
  type('-3');
  entry.input.dispatch('keydown', { key: 'Enter' });
  assert.deepEqual(applied, [0]);
  assert.ok(entry.isOpen);
  t.mock.timers.tick(NOTE_MS);
  assert.equal(entry.isOpen, false);
  assert.deepEqual(notes, ['Clamped to the minimum, 0.000', null]);
});

test('invalid text stays open, marked, and sends nothing', () => {
  const { entry, applied, type } = entryOn();
  entry.open();
  type('abc');
  entry.input.dispatch('keydown', { key: 'Enter' });
  assert.ok(entry.isOpen);
  assert.equal(entry.input.getAttribute('aria-invalid'), 'true');
  assert.match(entry.input.title, /not a number/);
  assert.deepEqual(applied, []);
});

// auditor-1-001: an editable row is built, and its base cell takes a value.
test('a Parameters row is built and its value cell sends a typed value as a release', async () => {
  const { SignalAppParameters } = await import('../app/app.js');
  const sent = [];
  const owner = {
    cells: new Map(),
    request(message) {
      sent.push(message);
      return Promise.resolve();
    },
  };
  const p = { editable: true, name: 'gain', address: '/out.gain', unit: null, min: 0, max: 4, default: 1, base: 1, inFile: true, taper: 'db', kind: 'gain' };
  const row = SignalAppParameters.prototype.row.call(owner, p);
  assert.equal(row.tagName, 'tr');
  const cells = owner.cells.get('/out.gain');
  assert.ok(cells.mark && cells.entry, 'the row has its mark and its value entry');
  cells.base.dispatch('dblclick');
  assert.ok(cells.entry.isOpen);
  assert.equal(cells.entry.input.value, '1');
  cells.entry.input.value = '-6 dB';
  cells.entry.input.dispatch('input');
  cells.entry.input.dispatch('keydown', { key: 'Enter' });
  const gain = 10 ** (-6 / 20);
  assert.deepEqual(sent, [
    { type: 'ramp', address: '/out.gain', to: gain, ms: 5 },
    { type: 'commit', address: '/out.gain', value: gain },
  ]);
  assert.equal(cells.entry.isOpen, false);
  cells.slider.dispatch('keydown', { key: 'Enter', preventDefault() {} });
  assert.ok(cells.entry.isOpen, 'Enter on the slider opens it');
});

// auditor-1-002: a control without a readout connects, and takes nothing typed.
test('a toggle connects without a value entry; a control with a readout gets one', async () => {
  const { SignalElement, SignalValueElement } = await import('./base.js');
  const { SignalToggle } = await import('./button.js');
  const { SignalFader } = await import('./fader.js');
  const original = SignalElement.prototype.connectedCallback;
  SignalElement.prototype.connectedCallback = function () {};
  try {
    const toggle = { constructor: SignalToggle, binding: { address: '/synth.on' }, redrawUnsaved() {}, $: () => null };
    SignalValueElement.prototype.connectedCallback.call(toggle);
    assert.equal(toggle.entry, undefined);
    const fader = { constructor: SignalFader, binding: { address: '/out.gain' }, redrawUnsaved() {}, $: () => new FakeElement() };
    SignalValueElement.prototype.connectedCallback.call(fader);
    assert.ok(fader.entry instanceof ValueEntry);
  } finally {
    SignalElement.prototype.connectedCallback = original;
  }
});

// auditor-1-003: a double-click on the pad's readout moves and writes nothing.
test('a double-click on the 2D pad readout writes nothing before its field opens', async () => {
  const { SignalXY } = await import('./xy.js');
  const pad = new FakeElement();
  const readout = new FakeElement();
  const writes = [];
  let opened = 0;
  Object.assign(pad, {
    $: (selector) => (selector === '.readout' ? readout : null),
    capture() {},
    pointerPositions: () => [0.75, 0],
    update() {},
    moveBoth: (...args) => writes.push(['move', ...args]),
    x: { value: 0.4, setValue: (...args) => writes.push(['x', ...args]) },
    y: { value: 0.6, setValue: (...args) => writes.push(['y', ...args]) },
    entries: [{ at: () => true, open: () => ++opened > 0 }],
  });
  SignalXY.prototype.setup.call(pad);
  const onReadout = { button: 0, shiftKey: false, composedPath: () => [readout, pad] };
  for (let i = 0; i < 2; i += 1) {
    pad.dispatch('pointerdown', onReadout);
    pad.dispatch('pointerup');
  }
  pad.dispatch('dblclick', {});
  assert.deepEqual(writes, [], 'no ramp, no commit');
  assert.equal(opened, 1, 'the field opened');
  pad.dispatch('pointerdown', { button: 0, shiftKey: false, composedPath: () => [new FakeElement(), pad] });
  assert.deepEqual(writes, [['move', 0.75, 0, 'ramp']], 'a press on the pad still moves it');
});
