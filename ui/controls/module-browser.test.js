// The Add module browser's element (docs/ui-controls.md "Editing the
// patch") on the least of a DOM it needs: it opens as a modal, lists the
// tree, moves with the keys, shows a type's detail and previews, adds by
// Enter and by a double-click, and closes on Escape without its keys
// reaching the schematic (⌘ and Ctrl chords but undo and redo go on to the
// app); and the editor that opens it (./schematic-editor.js): `modules`
// asked with `inside` once per state of the files, a name refused or written.

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
    this.classList = { add: (c) => classes.add(c), remove: (c) => classes.delete(c), toggle: (c, on = !classes.has(c)) => (on ? classes.add(c) : classes.delete(c), on), contains: (c) => classes.has(c) || String(this.className ?? '').split(' ').includes(c) };
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
  prepend(...nodes) {
    for (const node of nodes) node.parent = this;
    this.children.unshift(...nodes);
  }
  toggleAttribute(name, on) {
    if (on) this.attributes.set(name, '');
    else this.attributes.delete(name);
  }
  replaceChildren(...nodes) {
    this.children = [];
    this.append(...nodes);
  }
  all() {
    return this.children.flatMap((c) => [c, ...(c.all?.() ?? [])]);
  }
  querySelectorAll() {
    return this.all();
  }
  closest(selector) {
    const match = /^li\[data-key\]$/.test(selector);
    for (let at = this; at; at = at.parent) if (match && at.localName === 'li' && at.dataset.key) return at;
    return null;
  }
  focus() {
    document.activeElement = this;
  }
  scrollIntoView() {}
  showModal() {
    this.open = true;
  }
  close() {
    this.open = false;
  }
  /** `type` dispatched at this node and bubbled up (stopPropagation honoured). */
  dispatch(type, init = {}) {
    const event = { type, target: this, stopped: false, prevented: false, stopPropagation() { this.stopped = true; }, preventDefault() { this.prevented = true; }, ...init };
    for (let at = this; at && !event.stopped; at = at.parent) for (const fn of at.listeners[type] ?? []) fn(event);
    return event;
  }
}

const FAKE = {
  activeElement: null,
  createElement: (tag) => new Node(tag),
  createElementNS: (_, tag) => new Node(tag),
  createTextNode: (text) => Object.assign(new Node('#text'), { text: String(text), nodeType: 3 }),
};
const document = FAKE;
globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };

/** A test body with the stand-in as the page's document (only while it runs: the other tests of `node --test ui/` have none). */
function withDocument(body) {
  return () => {
    const before = Object.getOwnPropertyDescriptor(globalThis, 'document');
    globalThis.document = FAKE;
    try {
      body();
    } finally {
      if (before) Object.defineProperty(globalThis, 'document', before);
      else delete globalThis.document;
    }
  };
}

const { ModuleBrowser } = await import('./module-browser.js');
const { EXPANDED, browserEntries } = await import('../lib/module-browser.js');

const MODULES = {
  primitives: [
    {
      module: 'std.osc',
      composite: false,
      category: 'generator',
      description: 'An oscillator of four shapes. Its phase restarts on reset.',
      ports: [{ name: 'out', direction: 'out', shape: 'signal', rate: 1, kind: 'audio', unit: null, range: null }],
      parameters: [
        { name: 'freq', kind: 'frequency', rate: 1, unit: 'Hz', range: { min: 0, max: 24000 }, default: 440, combine: 'add' },
        { name: 'wave', kind: 'control', rate: 1, unit: null, range: { min: 0, max: 1 }, default: 1, combine: 'replace', values: [{ value: 0, label: 'sine' }, { value: 1, label: 'saw' }] },
      ],
    },
    { module: 'std.add', composite: false, category: 'core', description: 'Adds its two signal inputs: out = a + b.', ports: [], parameters: [] },
  ],
  composites: [
    {
      module: 'voice',
      composite: true,
      description: 'A voice.',
      ports: [{ name: 'out', direction: 'out', shape: 'signal', rate: 1, kind: 'audio' }],
      parameters: [],
      inside: { instances: [{ name: 'osc', module: 'std.osc', composite: false, ports: [{ name: 'out', direction: 'out', shape: 'signal' }], parameters: [] }], connections: [], inputs: [], outputs: [{ name: 'out', from: 'osc.out', shape: 'signal' }], parameters: [] },
    },
  ],
};

const key = (name, extra = {}) => ({ key: name, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...extra });

function browser() {
  const added = [];
  const closed = [];
  const b = new ModuleBrowser({
    add: (module, name) => {
      if (name === 'taken') return '`taken` is already an instance here';
      added.push([module, name]);
      b.close();
      return null;
    },
    closed: () => closed.push(true),
    graph: () => null,
    defaultName: (module) => module.split('.').pop(),
  });
  const host = new Node('signal-schematic');
  host.append(b.build(EXPANDED));
  const schematicKeys = [];
  host.addEventListener('keydown', (event) => schematicKeys.push(event.key));
  return { b, added, closed, schematicKeys };
}

/** The tree's rows as `<level> <label>`, the cursor's marked `>`. */
function rows(b) {
  return b.list.children.map((li) => `${li.getAttribute('aria-selected') === 'true' ? '>' : ''}${li.getAttribute('aria-level')} ${li.children[1]?.textContent ?? li.textContent}`);
}

test('it opens as a modal, loading, then lists the tree with the first type chosen and its detail', withDocument(() => {
  const { b } = browser();
  b.open({ where: 'at the top level' });
  assert.equal(b.isOpen, true);
  assert.equal(b.count.textContent, 'Loading…');
  assert.equal(document.activeElement, b.search, 'the search field has the focus');
  b.setEntries(browserEntries(MODULES));
  assert.deepEqual(rows(b), ['1 This project', '>2 voice', '1 Standard primitives', '2 core', '2 generator']);
  assert.equal(b.count.textContent, '3 types');
  assert.equal(b.name.value, 'voice');
  const text = b.detailPane.textContent;
  assert.match(text, /A voice\./);
  assert.match(text, /Its inside: 1 instance, 0 connections/);
  assert.equal(b.search.getAttribute('aria-activedescendant'), b.list.children[1].id);
}));

test('the keys move the cursor, open a branch and step into it; the detail shows ranges and a value set', withDocument(() => {
  const { b, schematicKeys } = browser();
  b.open({ where: '', entries: browserEntries(MODULES) });
  for (const name of ['ArrowDown', 'ArrowDown', 'ArrowDown']) b.search.dispatch('keydown', key(name));
  assert.deepEqual(rows(b).at(-1), '>2 generator');
  b.search.dispatch('keydown', key('ArrowRight'));
  assert.deepEqual(rows(b).slice(-2), ['>2 generator', '3 std.osc'], 'opened');
  b.search.dispatch('keydown', key('ArrowRight'));
  assert.deepEqual(rows(b).at(-1), '>3 std.osc', 'into its first row');
  assert.deepEqual(schematicKeys, [], 'no key reaches the schematic');
  const text = b.detailPane.textContent;
  assert.match(text, /Standard primitives · generator · 0 inputs, 1 output, 2 parameters/);
  assert.match(text, /An oscillator of four shapes\. Its phase restarts on reset\./, 'the full description');
  assert.match(text, /\[0, 24000\]/);
  assert.match(text, /0 → sine1 → saw/);
  assert.match(text, /1 \(saw\)/);
  b.search.dispatch('keydown', key('ArrowLeft'));
  assert.deepEqual(rows(b).at(-2), '>2 generator', 'out to its branch');
}));

test('a search filters the tree; Enter adds the type under the cursor with its name', withDocument(() => {
  const { b, added, closed } = browser();
  b.open({ where: '', entries: browserEntries(MODULES) });
  b.search.value = 'out = a';
  b.search.dispatch('input');
  assert.deepEqual(rows(b), ['1 Standard primitives', '2 core', '>3 std.add']);
  assert.equal(b.count.textContent, '1 match');
  b.name.value = 'taken';
  b.search.dispatch('keydown', key('Enter'));
  assert.equal(b.why.textContent, '`taken` is already an instance here', 'a refused name says why and stays open');
  assert.equal(b.isOpen, true);
  b.name.value = 'sum';
  b.search.dispatch('keydown', key('Enter'));
  assert.deepEqual(added, [['std.add', 'sum']]);
  assert.equal(b.isOpen, false);
  assert.equal(closed.length, 1);
}));

test('a double-click on a type adds it; a click on a branch opens it', withDocument(() => {
  const { b, added } = browser();
  b.open({ where: '', entries: browserEntries(MODULES) });
  b.list.children[2].dispatch('click'); // Standard primitives: closes
  assert.deepEqual(rows(b), ['1 This project', '2 voice', '>1 Standard primitives']);
  b.list.children[2].dispatch('click');
  b.list.children[3].dispatch('click'); // core: opens
  const add = b.list.children.find((li) => li.dataset.key === 'm:std.add');
  add.children[1].dispatch('click');
  add.children[1].dispatch('dblclick');
  assert.deepEqual(added, [['std.add', 'add']]);
}));

test('Escape closes it, adding nothing, and the schematic never sees the key', withDocument(() => {
  const { b, added, closed, schematicKeys } = browser();
  b.open({ where: '', entries: browserEntries(MODULES) });
  const event = b.search.dispatch('keydown', key('Escape'));
  assert.equal(event.prevented, true);
  assert.equal(b.isOpen, false);
  assert.deepEqual([added, closed.length, schematicKeys], [[], 1, []]);
}));

test('the panel preview is the generated card, inert, its controls on a connection that sends nothing', withDocument(() => {
  const { b } = browser();
  b.open({ where: '', entries: browserEntries(MODULES) });
  b.cursor = 'm:std.osc';
  b.expanded = new Set([...b.expanded, 'c:std/generator']);
  b.show();
  const grid = b.detailPane.all().find((n) => String(n.className).includes('generated'));
  assert.equal(grid.inert, true);
  const controls = grid.all().filter((n) => n.localName.startsWith('signal-'));
  assert.deepEqual(controls.map((c) => [c.localName, c.getAttribute('bind')]), [['signal-knob', '/osc.freq'], ['signal-knob', '/osc.wave']]);
  assert.equal(b.previewConnection.sent, 0);
  const svgs = b.detailPane.all().filter((n) => n.localName === 'svg');
  assert.equal(svgs.length, 1, 'a primitive: its box only');
}));

test('⌘ and Ctrl chords go on to the app, but undo and redo; an IME’s Enter and Escape are the field’s', withDocument(() => {
  const { b, added, schematicKeys } = browser();
  b.open({ where: '', entries: browserEntries(MODULES) });
  b.search.dispatch('keydown', key('s', { metaKey: true }));
  b.search.dispatch('keydown', key('o', { ctrlKey: true }));
  b.search.dispatch('keydown', key('z', { metaKey: true }));
  b.search.dispatch('keydown', key('z', { metaKey: true, shiftKey: true }));
  b.search.dispatch('keydown', key('a'));
  assert.deepEqual(schematicKeys, ['s', 'o'], 'only the app’s chords leave the dialog');
  const composing = b.search.dispatch('keydown', key('Enter', { isComposing: true }));
  assert.equal(composing.prevented, false);
  b.name.dispatch('keydown', key('Enter', { isComposing: true }));
  b.search.dispatch('keydown', key('Escape', { isComposing: true }));
  assert.deepEqual([added, b.isOpen], [[], true], 'nothing added, still open');
}));

test('a refused `modules` is said in the panes, not as Loading…', withDocument(() => {
  const { b } = browser();
  b.open({ where: '' });
  b.failed('the project does not load');
  assert.equal(b.count.textContent, 'Not loaded');
  assert.match(b.list.textContent, /No module types: the project does not load/);
  assert.match(b.detailPane.textContent, /could not be read: the project does not load/);
  b.search.value = 'x';
  b.search.dispatch('input');
  assert.doesNotMatch(b.count.textContent + b.detailPane.textContent, /Loading/, 'a search keeps saying so');
  b.setEntries(browserEntries(MODULES));
  assert.equal(b.count.textContent, '0 matches');
}));

test('a click on a branch during a search keeps the type chosen; a double-click on a branch toggles it once', withDocument(() => {
  const { b } = browser();
  b.open({ where: '', entries: browserEntries(MODULES) });
  b.search.value = 'osc';
  b.search.dispatch('input');
  assert.equal(b.chosen.module, 'std.osc');
  const branch = b.list.children.find((li) => li.dataset.key === 's:std');
  branch.dispatch('click', { detail: 1 });
  assert.deepEqual([b.cursor, b.chosen.module], ['m:std.osc', 'std.osc']);
  b.search.value = '';
  b.search.dispatch('input');
  const before = b.expanded.has('s:project');
  const project = () => b.list.children.find((li) => li.dataset.key === 's:project');
  project().dispatch('click', { detail: 1 });
  project().dispatch('click', { detail: 2 });
  project().dispatch('dblclick', { detail: 2 });
  assert.equal(b.expanded.has('s:project'), !before, 'toggled once');
}));

test('the previews follow the name typed in the footer', withDocument(() => {
  const { b } = browser();
  b.open({ where: '', entries: browserEntries(MODULES) });
  b.cursor = 'm:std.osc';
  b.expanded = new Set([...b.expanded, 'c:std/generator']);
  b.show();
  const binds = () => b.detailPane.all().filter((n) => n.localName === 'signal-knob').map((n) => n.getAttribute('bind'));
  assert.deepEqual(binds(), ['/osc.freq', '/osc.wave']);
  b.name.value = 'lead';
  b.name.dispatch('input');
  assert.deepEqual(binds(), ['/lead.freq', '/lead.wave']);
  assert.match(b.detailPane.all().find((n) => n.localName === 'svg').textContent, /^lead/);
  b.name.value = 'Not a name';
  b.name.dispatch('input');
  assert.deepEqual(binds(), ['/osc.freq', '/osc.wave'], 'not a name: the type’s own');
}));

const { SchematicEditor } = await import('./schematic-editor.js');

/** An editor on a stand-in schematic whose connection answers `modules` with MODULES (or refuses), counting what it asks. */
function editor({ refuse = false } = {}) {
  const asked = [];
  const focused = [];
  const graph = { project: 'p', projectId: 'id', files: { '': 'aa' }, top: { instances: [{ name: 'osc' }], connections: [] }, composites: {} };
  const s = {
    graph,
    drawn: null,
    connection: {
      request(message) {
        asked.push(message);
        return refuse ? Promise.reject(new Error('the project does not load')) : Promise.resolve(MODULES);
      },
    },
    focus: (options) => focused.push(options),
    toggleAttribute() {},
    showSide() {},
    say() {},
    modulesSeen() {},
  };
  const e = new SchematicEditor(s);
  e.build(new Node('div'), new Node('div'));
  e.setEditing(true);
  focused.length = 0;
  const written = [];
  e.write = (step, level, ops) => written.push([step, level, ops]);
  return { e, s, asked, focused, written };
}

const LEVEL = { level: '', transform: { k: 1, x: 0, y: 0 } };
const settle = () => new Promise((resolve) => setImmediate(resolve));

/** An async test body with the stand-in as the page's document while it runs. */
function withDocumentAsync(body) {
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

test('the editor asks for `modules` with `inside` once per state of the files, and shows the last reply at once', withDocumentAsync(async () => {
  const { e, s, asked } = editor();
  e.openPalette({ x: 10, y: 20 }, LEVEL);
  assert.deepEqual(asked, [{ type: 'modules', inside: true }]);
  assert.equal(e.browser.entries, null, 'nothing known yet');
  await settle();
  assert.equal(e.browser.entries.length, 3);
  e.closePalette();
  e.openPalette({ x: 10, y: 20 }, LEVEL);
  assert.equal(asked.length, 1, 'the same files: nothing asked');
  assert.equal(e.browser.entries.length, 3, 'shown at once');
  e.closePalette();
  s.graph = { ...s.graph, files: { '': 'bb' } };
  e.openPalette({ x: 10, y: 20 }, LEVEL);
  assert.equal(asked.length, 2, 'a file changed: asked again');
  assert.equal(e.browser.entries, null);
  await settle();
  assert.equal(e.browser.entries.length, 3);
}));

test('the editor refuses a taken name in the browser, writes a free one where it opened, and gives the focus back', withDocumentAsync(async () => {
  const { e, focused, written } = editor();
  e.openPalette({ x: 10, y: 20 }, LEVEL);
  await settle();
  const b = e.browser;
  b.cursor = 'm:std.osc';
  b.expanded = new Set([...b.expanded, 'c:std/generator']);
  b.show();
  assert.equal(b.name.value, 'osc_2', 'the lowest free name');
  b.name.value = 'osc';
  b.name.dispatch('keydown', key('Enter'));
  assert.match(b.why.textContent, /osc/);
  assert.deepEqual([written, b.isOpen, focused.length], [[], true, 0], 'refused: nothing written, still open');
  b.name.value = 'lead';
  b.name.dispatch('keydown', key('Enter'));
  assert.deepEqual(written, [[0, '', [{ op: 'instance.add', name: 'lead', module: 'std.osc', x: 10, y: 20 }]]]);
  assert.equal(b.isOpen, false);
  assert.equal(e.placing, null);
  assert.equal(focused.length, 1, 'the focus back on the schematic');
  // Cancel: nothing written, the focus back too.
  e.openPalette({ x: 0, y: 0 }, LEVEL);
  b.dialog.dispatch('cancel');
  assert.deepEqual([written.length, focused.length, b.isOpen], [1, 2, false]);
}));

test('a refused `modules` reaches the browser', withDocumentAsync(async () => {
  const { e } = editor({ refuse: true });
  e.openPalette({ x: 0, y: 0 }, LEVEL);
  await settle();
  assert.equal(e.browser.failure, 'the project does not load');
  assert.equal(e.browser.count.textContent, 'Not loaded');
}));
