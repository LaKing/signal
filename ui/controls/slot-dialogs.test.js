// The slot dialogs (ADR 0037 phase 3, docs/ui-controls.md "Slots") on the
// least of a DOM they need: Add slot writes its name and template; the
// interface editor turns its rows' edits into one slot.interface op and
// stays open with the server's refusal; the map editor writes slot.map;
// a template's name is checked before it is saved; Escape closes without
// the key reaching the schematic.

import test from 'node:test';
import assert from 'node:assert/strict';

class Node {
  constructor(tag) {
    this.localName = tag;
    this.nodeType = 1;
    this.attributes = new Map();
    this.listeners = {};
    this.children = [];
    this.parent = null;
    this.style = {};
    this.dataset = {};
    this.text = '';
    const classes = new Set();
    this.classList = { add: (c) => classes.add(c), remove: (c) => classes.delete(c), toggle: (c, on = !classes.has(c)) => (on ? classes.add(c) : classes.delete(c), on), contains: (c) => classes.has(c) };
  }
  set textContent(value) {
    this.children = [];
    this.text = String(value);
  }
  get textContent() {
    return this.text + this.children.map((c) => c.textContent).join('');
  }
  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }
  getAttribute(name) {
    return this.attributes.get(name) ?? null;
  }
  removeAttribute(name) {
    this.attributes.delete(name);
  }
  addEventListener(type, fn) {
    (this.listeners[type] ??= []).push(fn);
  }
  append(...nodes) {
    for (const node of nodes) {
      node.parent = this;
      this.children.push(node);
    }
  }
  replaceChildren(...nodes) {
    this.children = [];
    this.append(...nodes);
  }
  all() {
    return this.children.flatMap((c) => [c, ...(c.all?.() ?? [])]);
  }
  showModal() {
    this.open = true;
  }
  close() {
    this.open = false;
  }
  dispatch(type, init = {}) {
    const event = { type, target: this, stopped: false, prevented: false, stopPropagation() { this.stopped = true; }, preventDefault() { this.prevented = true; }, ...init };
    for (let at = this; at && !event.stopped; at = at.parent) for (const fn of at.listeners[type] ?? []) fn(event);
    return event;
  }
}

const FAKE = {
  activeElement: null,
  createElement: (tag) => new Node(tag),
  createTextNode: (text) => Object.assign(new Node('#text'), { text: String(text), nodeType: 3 }),
};

function withDocument(body) {
  return async () => {
    const before = Object.getOwnPropertyDescriptor(globalThis, 'document');
    globalThis.document = FAKE;
    try {
      await body();
    } finally {
      if (before) Object.defineProperty(globalThis, 'document', before);
      else delete globalThis.document;
    }
  };
}

const { SlotDialogs } = await import('./slot-dialogs.js');

const FX = {
  interface: {
    ports: [
      { name: 'x', role: 'input', shape: 'signal', rate: 1, kind: 'audio' },
      { name: 'y', role: 'output', shape: 'signal', rate: 1, kind: 'audio' },
    ],
  },
  from: null,
  load: 'std.gain',
  map: { x: 'in' },
};

function dialogs() {
  const closed = [];
  const d = new SlotDialogs({ closed: () => closed.push(true) });
  const host = new Node('signal-schematic');
  host.append(d.build());
  const keys = [];
  host.addEventListener('keydown', (event) => keys.push(event.key));
  return { d, closed, keys };
}

const find = (d, predicate) => d.dialog.all().find(predicate);
const button = (d, label) => find(d, (n) => n.localName === 'button' && n.textContent === label);
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

test('Add slot: its name and the template it starts from, written by add', withDocument(async () => {
  const { d } = dialogs();
  const added = [];
  d.openAddSlot({ where: 'At the top level', choices: [{ value: '', label: 'none' }, { value: 'lib.voice', label: 'lib.voice' }], names: new Set(['osc']), defaultName: 'slot', add: (name, template) => (added.push([name, template]), Promise.resolve({ ok: true })) });
  assert.equal(d.isOpen, true);
  const name = find(d, (n) => n.localName === 'input');
  const template = find(d, (n) => n.localName === 'select');
  name.value = 'osc';
  button(d, 'Add slot').dispatch('click');
  assert.match(find(d, (n) => n.className === 'why').textContent, /already has an instance osc/, 'a taken name is refused here');
  name.value = 'spare';
  template.value = 'lib.voice';
  button(d, 'Add slot').dispatch('click');
  await settle();
  assert.deepEqual(added, [['spare', 'lib.voice']]);
  assert.equal(d.isOpen, false, 'closed once written');
}));

test('the interface editor: a rename, a new port and a removal make one op; a refusal keeps it open', withDocument(async () => {
  const { d } = dialogs();
  const sent = [];
  let answer = { ok: false, error: 'in-use: wires name it' };
  d.openInterface({ slotName: 'fx', slot: FX, apply: (op) => (sent.push(op), Promise.resolve(answer)) });
  const names = () => d.dialog.all().filter((n) => n.localName === 'input' && n.className === 'name');
  assert.deepEqual(names().map((n) => n.value), ['x', 'y']);
  const x = names()[0];
  x.value = 'input';
  x.dispatch('input');
  button(d, '+ parameter').dispatch('click');
  assert.deepEqual(names().map((n) => n.value), ['input', 'y', 'param']);
  // Remove y (its row's ×).
  const removes = d.dialog.all().filter((n) => n.localName === 'button' && n.textContent === '×');
  removes[1].dispatch('click');
  assert.deepEqual(names().map((n) => n.value), ['input', 'param']);
  button(d, 'Apply').dispatch('click');
  await settle();
  assert.deepEqual(sent, [{ op: 'slot.interface', slot: 'fx', remove: ['y'], rename: { x: 'input' }, add: [{ name: 'param', role: 'parameter' }] }]);
  assert.equal(d.isOpen, true, 'refused: still open');
  assert.equal(find(d, (n) => n.className === 'why').textContent, 'in-use: wires name it');
  answer = { ok: true };
  button(d, 'Apply').dispatch('click');
  await settle();
  assert.equal(d.isOpen, false);
}));

test('the interface editor opened by a "+" pin has a new output; a bad name is said, not sent', withDocument(async () => {
  const { d } = dialogs();
  const sent = [];
  d.openInterface({ slotName: 'fx', slot: FX, focus: { add: 'out' }, apply: (op) => (sent.push(op), Promise.resolve({ ok: true })) });
  const names = d.dialog.all().filter((n) => n.localName === 'input' && n.className === 'name');
  assert.deepEqual(names.map((n) => n.value), ['x', 'y', 'out']);
  names[2].value = 'Bad';
  names[2].dispatch('input');
  button(d, 'Apply').dispatch('click');
  await settle();
  assert.deepEqual(sent, []);
  assert.match(find(d, (n) => n.className === 'why').textContent, /lower-case/);
}));

test('the map editor writes slot.map; Escape closes without reaching the schematic', withDocument(async () => {
  const { d, keys, closed } = dialogs();
  const sent = [];
  d.openMap({ slotName: 'fx', load: 'std.gain', rows: [{ port: 'x', role: 'input', value: 'in', options: ['in'] }, { port: 'y', role: 'output', value: '', options: ['out'] }], apply: (op) => (sent.push(op), Promise.resolve({ ok: true })) });
  const selects = d.dialog.all().filter((n) => n.localName === 'select');
  selects[1].value = 'false';
  selects[1].dispatch('change');
  button(d, 'Apply').dispatch('click');
  await settle();
  assert.deepEqual(sent, [{ op: 'slot.map', slot: 'fx', map: { x: 'in', y: false } }]);
  d.openMap({ slotName: 'fx', load: null, rows: [], apply: () => Promise.resolve({ ok: true }) });
  d.dialog.dispatch('keydown', { key: 'Escape', isComposing: false, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false });
  assert.equal(d.isOpen, false);
  assert.deepEqual(keys, [], 'the key stayed in the dialog');
  assert.ok(closed.length >= 2);
}));

test('a template’s name is checked: a bad one or one the project has is said, a good one saved', withDocument(async () => {
  const { d } = dialogs();
  const saved = [];
  d.openTemplateName({ title: 'Save', note: '', defaultName: 'bus', taken: new Set(['bus']), save: (name, description) => (saved.push([name, description]), Promise.resolve({ ok: true })) });
  button(d, 'Save').dispatch('click');
  assert.match(find(d, (n) => n.className === 'why').textContent, /exists already/);
  const [name, description] = d.dialog.all().filter((n) => n.localName === 'input');
  name.value = 'deck2';
  description.value = 'Two decks';
  button(d, 'Save').dispatch('click');
  await settle();
  assert.deepEqual(saved, [['deck2', 'Two decks']]);
}));
