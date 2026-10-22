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

// 2026-10-22: the double-click is told from the presses, not from `dblclick`
// alone (a captured pointer's `dblclick` is the browser's to target).

test('two quick presses on a readout open its field; slow ones do not', () => {
  const { readout, entry } = entryOn();
  const press = (timeStamp) => {
    readout.dispatch('pointerdown', { button: 0, timeStamp, clientX: 5, clientY: 5 });
    readout.dispatch('pointerup', { button: 0, timeStamp: timeStamp + 60, clientX: 5, clientY: 5 });
  };
  press(1000);
  press(2000);
  assert.equal(entry.isOpen, false, 'a second apart is two clicks');
  press(2200);
  assert.ok(entry.isOpen, 'the pair opens it, on its release');
  let stopped = false;
  readout.dispatch('dblclick', { stopPropagation: () => (stopped = true) });
  assert.ok(stopped, "the browser's dblclick that follows is the open field's, never a reset around it");
  assert.equal(readout.children.length, 1, 'one field');
});

test('a readout whose owner tells the double-click leaves the presses to it', () => {
  const readout = new FakeElement();
  const entry = new ValueEntry(readout, { pointer: false, spec: () => ({ unit: null, range: { min: 0, max: 1 } }), value: () => 0.5, apply() {}, note() {} });
  for (const timeStamp of [1000, 1100]) {
    readout.dispatch('pointerdown', { button: 0, timeStamp, clientX: 5, clientY: 5 });
    readout.dispatch('pointerup', { button: 0, timeStamp, clientX: 5, clientY: 5 });
  }
  assert.equal(entry.isOpen, false);
});

/** A knob-like control (`SignalValueElement.setup`) on stubs, recording what it does. */
async function valueControl({ entryOn: enabled = true } = {}) {
  const { SignalValueElement } = await import('./base.js');
  const control = new FakeElement();
  const did = [];
  let open = false;
  const proto = SignalValueElement.prototype;
  Object.assign(control, {
    capture() {},
    update() {},
    dragStart: (event) => did.push(['start', event.clientY]),
    dragMove: (event) => event.clientY / 100,
    dragEnd: proto.dragEnd,
    typeOrReset: proto.typeOrReset,
    resetValue: proto.resetValue,
    binding: {
      value: 0.5,
      moveTo: (position, how) => did.push(['move', position, how]),
      setValue: (value, how) => did.push(['set', value, how]),
      reset: () => did.push(['reset']),
    },
    entry: {
      at: (event) => Boolean(event.onReadout),
      open: () => {
        if (!enabled || open) return false;
        open = true;
        did.push(['open']);
        return true;
      },
      get isOpen() {
        return open;
      },
    },
  });
  proto.setup.call(control);
  const press = (timeStamp, { onReadout = false, y = 50, upY = y } = {}) => {
    control.dispatch('pointerdown', { button: 0, timeStamp, clientX: 20, clientY: y, onReadout });
    if (upY !== y) control.dispatch('pointermove', { clientX: 20, clientY: upY, onReadout });
    control.dispatch('pointerup', { button: 0, timeStamp: timeStamp + 50, clientX: 20, clientY: upY, onReadout });
  };
  return { control, did, press };
}

test('a double-click on a knob or fader readout opens the field and moves nothing, even with a little jitter', async () => {
  const { control, did, press } = await valueControl();
  press(1000, { onReadout: true, y: 50, upY: 52 });
  press(1150, { onReadout: true, y: 51, upY: 53 });
  assert.deepEqual(did, [['open']], 'no drag start, no move, no commit, no reset');
  control.dispatch('dblclick', { onReadout: true });
  assert.deepEqual(did, [['open']], "the browser's dblclick after it does nothing more");
});

test('a double-click elsewhere on the control resets, once', async () => {
  const { control, did, press } = await valueControl();
  press(1000);
  press(1200);
  control.dispatch('dblclick', {});
  assert.deepEqual(
    did.filter(([what]) => what !== 'start'),
    [['reset']],
  );
});

test('with typed entry off, a double-click on the readout resets', async () => {
  const { did, press } = await valueControl({ entryOn: false });
  press(1000, { onReadout: true });
  press(1100, { onReadout: true });
  assert.deepEqual(did, [['reset']]);
});

test('a dblclick without presses (a script, an assistive tool) still types on the readout and resets elsewhere', async () => {
  const { control, did } = await valueControl();
  control.dispatch('dblclick', { onReadout: true });
  assert.deepEqual(did, [['open']]);
  const other = await valueControl();
  other.control.dispatch('dblclick', {});
  assert.deepEqual(other.did, [['reset']]);
});

test('a drag that starts on the readout moves the value once past its slack', async () => {
  const { control, did } = await valueControl();
  control.dispatch('pointerdown', { button: 0, timeStamp: 1000, clientX: 20, clientY: 50, onReadout: true });
  control.dispatch('pointermove', { clientX: 20, clientY: 52 });
  assert.deepEqual(did, [], 'within the slack');
  control.dispatch('pointermove', { clientX: 20, clientY: 60 });
  control.dispatch('pointermove', { clientX: 20, clientY: 70 });
  control.dispatch('pointerup', { button: 0, timeStamp: 1400, clientX: 20, clientY: 70 });
  assert.deepEqual(did, [['start', 60], ['move', 0.7, 'ramp'], ['set', 0.5, 'commit']]);
});

test("the scope's trigger level is typed on its readout, a decimal comma or dot", async () => {
  const { SignalScope } = await import('./scope.js');
  const readout = new FakeElement();
  const scope = new FakeElement();
  Object.assign(scope, { $: () => readout, xy: false, problemText: null, update() {} });
  Object.defineProperty(scope, 'level', { get: () => Number(scope.getAttribute('trigger-level') ?? 0) });
  SignalScope.prototype.setup.call(scope);
  readout.dispatch('dblclick');
  assert.equal(scope.entry.input.value, '0');
  scope.entry.input.value = '0,25';
  scope.entry.input.dispatch('input');
  scope.entry.input.dispatch('keydown', { key: 'Enter' });
  assert.equal(scope.getAttribute('trigger-level'), '0.25');
  scope.xy = true;
  readout.dispatch('dblclick');
  assert.equal(scope.entry.isOpen, false, 'none in X–Y mode');
});
