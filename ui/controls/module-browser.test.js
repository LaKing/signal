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
    this.classSet = classes;
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
  createElementNS: (ns, tag) => Object.assign(new Node(tag), { ns }),
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
const slotLib = await import('../lib/slot.js');

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

test('ADR 0037 §7: the Load dialog: titled, no name, each row its match, the unmatched ports offered with Load', withDocument(() => {
  const { contentMatch } = slotLib;
  const loaded = [];
  // A slot with one output `out` and an input `in` nothing here offers.
  const slot = { interface: { ports: [{ name: 'out', role: 'output' }, { name: 'trig', role: 'input' }] } };
  const b = new ModuleBrowser({
    mode: 'load',
    add: (module, _name, options) => {
      loaded.push([module, options.addUnmatched, options.entry.module]);
      b.close();
      return null;
    },
    closed: () => {},
    graph: () => null,
    defaultName: () => '',
    matchOf: (entry) => contentMatch(entry, slot),
  });
  new Node('signal-slot').append(b.build([...EXPANDED, 's:library', 's:folders']));
  b.open({ where: '', entries: browserEntries(MODULES), title: 'Load into fx' });
  assert.equal(b.heading.textContent, 'Load into fx');
  assert.equal(b.addButton.textContent, 'Load');
  assert.equal(b.dialog.getAttribute('aria-label'), 'Load into slot');
  const voice = b.list.children.find((li) => li.children[1]?.textContent === 'voice');
  assert.match(voice.children[2].textContent, /^1\/2 match/, 'voice offers `out`');
  // std.osc matches `out` too; std.add nothing: it is ranked last in its branch.
  assert.equal(b.entries.at(-1).module, 'std.add');
  assert.match(b.detailPane.textContent, /1 of the slot’s 2 ports and parameters match by name/);
  assert.equal(b.unmatchedLabel.hidden, true, 'voice offers nothing the slot lacks');
  b.unmatchedBox.checked = true;
  b.addButton.dispatch('click');
  assert.deepEqual(loaded, [['voice', true, 'voice']]);
  assert.equal(b.isOpen, false);
}));

// 2026-10-27, the owner in Signal.app: the tree's types never showed (a
// branch opened to nothing under it; This project opened to nothing) and
// the search field stuck out of the frame, unusable. The rows were right;
// the CSS of the shadow root the browser shares was not: its leaves were
// `li.entry`, and `.entry` is the value field's (ENTRY_CSS: absolute,
// inset 0, 100% wide), in <signal-schematic>'s root and in every control's
// (BASE_CSS: the slot's, the Load dialog's other host). Each type's row was
// taken out of the list and laid at the top of the modal dialog (its
// containing block), over the search field. These check the rows the clicks
// show and that no rule of a host's own reaches an element of the browser.

const { readFileSync } = await import('node:fs');
const { ENTRY_CSS } = await import('./entry-field.js');
const { MENU_CSS } = await import('./context-menu.js');
const { EDITOR_CSS } = await import('./schematic-editor.js');
// ADR 0046 §5: the slot card edits its part with the panel editor, whose tools it styles.
const { EDITOR_CSS: PANEL_EDITOR_CSS } = await import('./panel-editor.js');
const { BROWSER_CSS } = await import('./module-browser.js');
const { SLOT_DIALOG_CSS, SlotDialogs } = await import('./slot-dialogs.js');

/** A CSS template literal of `file` (from `start`, the text before its backtick), its `${NAME}`s replaced by `known[NAME]`. */
function cssIn(file, start, known) {
  const text = readFileSync(new URL(file, import.meta.url), 'utf8');
  const at = text.indexOf(start);
  assert.ok(at >= 0, `${start} in ${file}`);
  const open = text.indexOf('`', at) + 1;
  const body = text.slice(open, text.indexOf('`;', open));
  return body.replace(/\$\{(\w+)\}/g, (_, name) => {
    assert.ok(name in known, `${name} known (in ${file})`);
    return known[name];
  });
}

/** The rules of `css` as `[selector, declarations]`, one per selector (comments and at-rules' heads dropped: a nested rule is its own). */
function rulesOf(css) {
  const flat = css.replace(/\/\*[\s\S]*?\*\//g, '').replace(/@[^{;]+\{/g, '');
  return [...flat.matchAll(/([^{}]+)\{([^{}]*)\}/g)].flatMap((m) => m[1].split(',').map((s) => [s.trim(), m[2]])).filter(([s]) => s);
}

/** The classes a selector of one compound names (no combinator: it matches its element wherever it is). */
function bareClasses(selector) {
  const plain = selector.replace(/\([^)]*\)|\[[^\]]*\]/g, '');
  if (/[\s>+~]/.test(plain)) return [];
  return [...plain.matchAll(/\.([\w-]+)/g)].map((m) => m[1]);
}

/** The host roots' CSS but the browser's own: <signal-schematic>'s and a control's (<signal-slot>, the Load dialog's other host). */
function hostCss() {
  const base = cssIn('./base.js', 'export const BASE_CSS', { ENTRY_CSS });
  const schematic = cssIn('./schematic.js', 'const CSS =', { ENTRY_CSS, EDITOR_CSS, MENU_CSS });
  const slot = cssIn('./slot.js', 'static css =', { BROWSER_CSS, EDITOR_CSS: PANEL_EDITOR_CSS });
  const own = (text) => text.replace(BROWSER_CSS, '').replace(SLOT_DIALOG_CSS, '');
  return { schematic: own(schematic), slot: base + own(slot) };
}

/**
 * The dialog's HTML elements as `tag.class.class` → `{classes, card}`; not the
 * previews' SVG (the schematic's own `.box`, `.pin`… draw it). `card`: in
 * the generated panel card of a preview, whose classes are panel.js's.
 */
function htmlClasses(dialog, found = new Map()) {
  const walk = (node, card) => {
    if (node.ns || node.nodeType !== 1) return;
    const classes = [...new Set([...String(node.className ?? '').split(/\s+/), ...node.classSet].filter(Boolean))];
    const inCard = card || classes.includes('generated');
    if (classes.length) found.set(`${inCard ? 'card ' : ''}${[node.localName, ...classes].join('.')}`, { classes, card: inCard && !classes.includes('generated') });
    for (const child of node.children) walk(child, inCard);
  };
  walk(dialog, false);
  return found;
}

/** The properties a declaration block sets. */
function propertiesOf(body) {
  return body.split(';').map((d) => d.split(':')[0].trim()).filter(Boolean);
}

/**
 * The host rules of one compound that reach an element of `seen`, as
 * `<selector> reaches <element>`: every one, but in a generated card (whose
 * classes are panel.js's, `.row` among them) a rule each of whose
 * properties a rule of `ownCss` scoped to `.generated` sets again for that
 * class (a shorthand, `margin`, covering its longhands).
 */
function reaching(seen, hostText, ownCss) {
  const rules = rulesOf(hostText).map(([s, body]) => [s, bareClasses(s), body]).filter(([, c]) => c.length);
  const own = rulesOf(ownCss).filter(([s]) => s.includes('.generated '));
  const undone = (need, body) =>
    propertiesOf(body).every((p) => own.some(([s, b]) => need.some((c) => s.endsWith(`.${c}`)) && propertiesOf(b).some((q) => q === p || p.startsWith(`${q}-`))));
  return [...seen].flatMap(([element, { classes, card }]) =>
    rules.filter(([, need, body]) => need.every((c) => classes.includes(c)) && !(card && undone(need, body))).map(([s]) => `${s} reaches ${element}`),
  );
}

const LIBRARY = {
  primitives: MODULES.primitives,
  composites: [
    ...MODULES.composites,
    { module: 'drums', composite: true, description: 'Drums.', ports: [], parameters: [], inside: { instances: [], connections: [], inputs: [], outputs: [], parameters: [] } },
    { module: 'lib.verb', composite: true, library: true, category: 'effect', description: 'A reverb.', ports: [], parameters: [] },
    { module: 'lib.echo', composite: true, library: true, category: 'effect', description: 'An echo.', ports: [], parameters: [] },
  ],
};

/** The list's rows as `<aria-level> <label>`. */
function shown(b) {
  return b.list.children.map((li) => `${li.getAttribute('aria-level')} ${li.children[1]?.textContent ?? li.textContent}`);
}

function clickRow(b, key) {
  b.list.children.find((li) => li.dataset.key === key).dispatch('click', { detail: 1 });
}

test('clicks open the branches down to their types: each a row of the list after its branch, nothing above it hidden', withDocument(() => {
  const { b } = browser();
  b.open({ where: '', entries: browserEntries(LIBRARY) });
  assert.deepEqual(shown(b), ['1 This project', '2 voice', '2 drums', '1 Library', '1 Standard primitives', '2 core', '2 generator'], 'This project open, its composites listed directly');
  for (const key of ['s:library', 'c:library/effect', 'c:std/generator', 'c:std/core']) clickRow(b, key);
  assert.deepEqual(shown(b), ['1 This project', '2 voice', '2 drums', '1 Library', '2 effect', '3 lib.verb', '3 lib.echo', '1 Standard primitives', '2 core', '3 std.add', '2 generator', '3 std.osc']);
  clickRow(b, 's:project');
  assert.deepEqual(shown(b).slice(0, 2), ['1 This project', '1 Library'], 'closed');
  clickRow(b, 's:project');
  assert.deepEqual(shown(b).slice(0, 3), ['1 This project', '2 voice', '2 drums'], 'and open again');
  const leaves = b.list.children.filter((n) => n.dataset.key?.startsWith('m:'));
  assert.equal(leaves.length, 6);
  for (const li of leaves) {
    assert.equal(li.parent, b.list, `${li.dataset.key} is a row of the list`);
    for (let at = li; at; at = at.parent) assert.ok(!at.hidden && at.getAttribute('hidden') === null && at.style.display !== 'none', `nothing hides ${li.dataset.key}`);
    assert.equal(li.className, 'tree-entry', 'a class of the browser’s own');
  }
  const source = b.list.children.find((n) => n.dataset.key === 's:library');
  assert.equal(source.className, 'tree-source');
  assert.equal(source.getAttribute('aria-expanded'), 'true');
}));

test('no rule of the host roots’ own reaches an element of the browser (the value field’s `.entry` took the types out of the tree)', withDocument(() => {
  const css = hostCss();
  // Each host's rules of one compound, by the classes an element must have to match.
  const bare = Object.fromEntries(Object.entries(css).map(([host, text]) => [host, rulesOf(text).map(([s]) => [s, bareClasses(s)]).filter(([, c]) => c.length)]));
  const names = (host) => new Set(bare[host].map(([s]) => s));
  assert.ok(names('schematic').has('.entry') && names('slot').has('.entry') && names('slot').has('.label') && names('slot').has('.head'), 'the parse finds the rules that collided');
  // Every element the browser builds: the tree open to its types, a type's detail and previews, no match; then the Load dialog's.
  const { b } = browser();
  b.open({ where: '', entries: browserEntries(LIBRARY) });
  for (const key of ['s:library', 'c:library/effect', 'c:std/generator']) clickRow(b, key);
  const seen = htmlClasses(b.dialog);
  clickRow(b, 'm:std.osc');
  htmlClasses(b.dialog, seen);
  b.search.value = 'nothing like it';
  b.search.dispatch('input');
  htmlClasses(b.dialog, seen);
  const loader = new ModuleBrowser({ mode: 'load', add: () => null, closed() {}, graph: () => null, defaultName: () => '', matchOf: () => ({ matched: 0, total: 1, unmatched: [{ name: 'x', role: 'input' }] }) });
  new Node('signal-slot').append(loader.build(EXPANDED));
  loader.open({ where: '', entries: browserEntries(LIBRARY) });
  htmlClasses(loader.dialog, seen);
  const all = new Set([...seen.values()].flatMap((e) => e.classes));
  for (const c of ['browser', 'tree-entry', 'tree-source', 'tree-category', 'tree-label', 'search', 'browser-head', 'browser-foot', 'col-name', 'preview-note', 'unmatched', 'generated', 'row']) assert.ok(all.has(c), `the walk saw .${c}`);
  for (const [host, text] of Object.entries(css)) assert.deepEqual(reaching(seen, text, BROWSER_CSS), [], `no rule of one compound in <signal-${host}>’s root reaches an element of the browser`);
}));

test('the frame holds the header: one column of minmax(0, 1fr), the search from a basis of 0, padding and border inside its width', () => {
  const rules = rulesOf(BROWSER_CSS);
  const decl = (selector, property) => {
    const bodies = rules.filter(([s]) => s === selector).map(([, body]) => body);
    assert.ok(bodies.length, `a rule for ${selector}`);
    const found = bodies.flatMap((body) => [...body.matchAll(new RegExp(`(?:^|;)\\s*${property}\\s*:\\s*([^;]+)`, 'g'))].map((m) => m[1].trim()));
    return found.at(-1) ?? null;
  };
  // A grid's implicit column is `auto`: at least its widest row's min-content (a table, a card, an input's own size), so the header could grow past the frame.
  assert.equal(decl('dialog.browser[open]', 'grid-template-columns'), 'minmax(0, 1fr)');
  assert.equal(decl('dialog.browser', 'box-sizing'), 'border-box');
  assert.equal(decl('dialog.browser', 'overflow'), 'hidden');
  // A field's padding and border are within its width; none keeps its intrinsic size as a minimum.
  assert.equal(decl('.browser input', 'box-sizing'), 'border-box');
  assert.equal(decl('.browser input', 'min-width'), '0');
  // The search takes the room left (flex from 0, not its intrinsic width), never wider than the header; WebKit's searchfield look off, so the padding and border hold.
  assert.equal(decl('.browser .browser-head .search', 'flex'), '1 1 0');
  assert.equal(decl('.browser .browser-head .search', 'width'), '0');
  assert.equal(decl('.browser .browser-head .search', 'max-width'), '100%');
  assert.equal(decl('.browser .browser-head .search', '-webkit-appearance'), 'none');
  assert.equal(decl('.browser .browser-head', 'flex-wrap'), 'nowrap');
  assert.equal(decl('.browser .browser-head', 'box-sizing'), 'border-box');
  // A row is in the list's flow, whatever else the root says.
  assert.equal(decl('.browser .tree li', 'position'), 'static');
  assert.equal(decl('.browser .tree li', 'width'), 'auto');
  // The narrow window: one column of panes, of minmax(0, 1fr) too.
  assert.match(BROWSER_CSS, /@media \(max-width: 720px\) \{\s*\.browser \.panes \{ grid-template-columns: minmax\(0, 1fr\);/);
});

test('no rule of the host roots’ own reaches an element of the slot dialogs (Add slot, Edit interface, Edit map, a template’s name), in either host', withDocument(() => {
  const FX = {
    interface: { ports: [{ name: 'x', role: 'input', shape: 'signal', rate: 1, kind: 'audio' }, { name: 'v', role: 'output', shape: { vector: 4 }, rate: 1, kind: 'audio' }, { name: 'gain', role: 'parameter', kind: 'control' }] },
    from: 'lib.fx',
    load: 'std.gain',
  };
  const seen = new Map();
  for (const host of ['signal-schematic', 'signal-slot']) {
    const d = new SlotDialogs({ closed() {} });
    new Node(host).append(d.build());
    const ok = () => Promise.resolve({ ok: true });
    d.openAddSlot({ where: 'At the top level', choices: [{ value: '', label: 'none' }, { value: 'lib.voice', label: 'lib.voice' }], names: new Set(['osc']), defaultName: 'slot', add: ok });
    htmlClasses(d.dialog, seen);
    d.openInterface({ slotName: 'fx', slot: FX, focus: { add: 'out' }, apply: ok, saveTemplate() {}, detach() {} });
    htmlClasses(d.dialog, seen);
    d.openMap({ slotName: 'fx', load: 'std.gain', rows: [{ port: 'x', role: 'input', value: 'in', options: ['in'] }, { port: 'v', role: 'output', value: false, options: ['out'] }], apply: ok });
    htmlClasses(d.dialog, seen);
    d.openMap({ slotName: 'fx', load: null, rows: [], apply: ok });
    htmlClasses(d.dialog, seen);
    d.openTemplateName({ title: 'Save', note: 'A template.', defaultName: 'bus', taken: new Set(['bus']), save: ok });
    htmlClasses(d.dialog, seen);
  }
  const all = new Set([...seen.values()].flatMap((e) => e.classes));
  for (const c of ['slot-dialog', 'dialog-head', 'dialog-note', 'dialog-foot', 'dialog-buttons', 'why', 'field', 'slot-name', 'port-name', 'small', 'unit', 'optional', 'adds', 'check', 'primary']) assert.ok(all.has(c), `the walk saw .${c}`);
  for (const [host, text] of Object.entries(hostCss())) assert.deepEqual(reaching(seen, text, SLOT_DIALOG_CSS), [], `no rule of one compound in <signal-${host}>’s root reaches an element of a slot dialog`);
}));
