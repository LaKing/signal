// <signal-plane> on a stub DOM (review of 2026-11-01). A slot instrument
// has one identity across validation, ownership, rendering and the
// boundary (ui/lib/plane-file.js `withEntries`): after Group into slot the
// slot's open card is the slot's instrument whether the Stack is derived
// or written, in its full form (`synth`) or its card's (`card:synth`); its
// front panel renders in the intended Stack, no instance of it lands in
// the unstacked tray, and no false edge pin appears. Space reaches the
// shared keymap's transport with the focus on the plane (ADR 0039), and
// the plane's own keys are commands of the keymap (scope `plane`).

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };

const { SignalPlane } = await import('./plane.js');
const { install } = await import('./keyboard.js');
const { applyKeys } = await import('../lib/keys.js');
const { setDefaultConnection } = await import('../signal-ui.js');

const whitelist = JSON.parse(readFileSync(new URL('../lib/panel-whitelist.json', import.meta.url), 'utf8'));

/** The least of a DOM the plane's Stacks need: elements with children, attributes, styles and a shadow root. */
class Node {
  constructor(name) {
    this.localName = name;
    this.children = [];
    this.parentElement = null;
    this.style = { setProperty() {} };
    this.dataset = {};
    this.attributes = new Map();
    this.text = '';
    this.listeners = [];
    const owner = this;
    this.classList = {
      add: (c) => owner.setAttribute('class', [...new Set([...owner.className.split(/\s+/).filter(Boolean), c])].join(' ')),
      remove: (c) => owner.setAttribute('class', owner.className.split(/\s+/).filter((x) => x && x !== c).join(' ')),
      toggle: (c, on) => ((on ?? !owner.classList.contains(c)) ? owner.classList.add(c) : owner.classList.remove(c)),
      contains: (c) => owner.className.split(/\s+/).includes(c),
    };
  }
  get className() {
    return this.attributes.get('class') ?? '';
  }
  set className(v) {
    this.attributes.set('class', String(v));
  }
  get textContent() {
    return this.text + this.children.map((c) => c.textContent).join('');
  }
  set textContent(v) {
    this.text = String(v);
    this.children = [];
  }
  get offsetHeight() {
    return 120;
  }
  get offsetWidth() {
    return 160;
  }
  setAttribute(k, v) {
    this.attributes.set(k, String(v));
  }
  getAttribute(k) {
    return this.attributes.get(k) ?? null;
  }
  hasAttribute(k) {
    return this.attributes.has(k);
  }
  removeAttribute(k) {
    this.attributes.delete(k);
  }
  addEventListener(type, fn) {
    this.listeners.push([type, fn]);
  }
  removeEventListener() {}
  append(...nodes) {
    for (const n of nodes) {
      if (typeof n === 'string') this.text += n;
      else {
        n.remove();
        n.parentElement = this;
        this.children.push(n);
      }
    }
  }
  insertBefore(n, ref) {
    n.remove();
    n.parentElement = this;
    const i = this.children.indexOf(ref);
    this.children.splice(i < 0 ? this.children.length : i, 0, n);
  }
  after(...nodes) {
    const parent = this.parentElement;
    if (!parent) return;
    let i = parent.children.indexOf(this) + 1;
    for (const n of nodes) {
      n.remove();
      n.parentElement = parent;
      parent.children.splice(i++, 0, n);
    }
  }
  replaceChildren(...nodes) {
    for (const c of this.children) c.parentElement = null;
    this.children = [];
    this.text = '';
    this.append(...nodes);
  }
  remove() {
    const parent = this.parentElement;
    if (!parent) return;
    parent.children.splice(parent.children.indexOf(this), 1);
    this.parentElement = null;
  }
  attachShadow() {
    this.shadowRoot = new Node('#shadow');
    return this.shadowRoot;
  }
  matches(selector) {
    if (selector.startsWith('.')) return this.classList.contains(selector.slice(1));
    if (selector === '[bind]') return this.hasAttribute('bind');
    return this.localName === selector;
  }
  all() {
    return this.children.flatMap((c) => [c, ...c.all()]);
  }
  querySelectorAll(selector) {
    return this.all().filter((n) => n.matches(selector));
  }
  querySelector(selector) {
    const scoped = /^:scope > (.*)$/.exec(selector);
    if (scoped) return this.children.find((c) => c.matches(scoped[1])) ?? null;
    return this.querySelectorAll(selector)[0] ?? null;
  }
  closest(selector) {
    for (let at = this; at; at = at.parentElement) if (at.matches(selector)) return at;
    return null;
  }
  getBoundingClientRect() {
    return { left: 0, top: 0, width: 160, height: 120, right: 160, bottom: 120 };
  }
}

globalThis.document = {
  createElement: (name) => new Node(name),
  createElementNS: (_ns, name) => new Node(name),
};
globalThis.requestAnimationFrame ??= (fn) => setTimeout(fn, 0);

// After Group into slot: `synth` is a slot with a part, its open slot card among the panel's cards.
const PANEL = [
  '<div class="signal-cards">',
  '<section class="signal-card"><h2>Melody</h2><signal-knob bind="/melody.rate"></signal-knob></section>',
  '<signal-slot bind="/synth"></signal-slot>',
  '<section class="signal-card"><h2>Echo</h2><signal-knob bind="/echo.mix"></signal-knob></section>',
  '</div>',
].join('');

const port = (instance, name, direction) => ({ address: `/${instance}.${name}`, name, direction, shape: 'signal' });
const GRAPH = {
  top: {
    instances: [
      { name: 'melody', module: 'std.note_seq', ports: [port('melody', 'out', 'out')], parameters: [] },
      { name: 'synth', module: 'lib.voice', slot: { part: { file: 'ui/parts/synth.html' } }, ports: [port('synth', 'in', 'in'), port('synth', 'out', 'out')], parameters: [] },
      { name: 'echo', module: 'std.delay', ports: [port('echo', 'in', 'in'), port('echo', 'out', 'out')], parameters: [] },
      { name: 'out', module: 'std.audio_out', ports: [port('out', 'l', 'in'), port('out', 'r', 'in')], parameters: [] },
    ],
    connections: [
      { from: '/melody.out', to: '/synth.in' },
      { from: '/synth.out', to: '/echo.in' },
      { from: '/echo.out', to: '/out.l' },
      { from: '/echo.out', to: '/out.r' },
    ],
  },
};

/** A plane without its build: the Stacks' layer real, the layout and drawing left out; the panel drawn as stub cards. */
function plane({ planeText = null } = {}) {
  const p = new SignalPlane();
  p.world = new Node('div');
  p.setupSvg = new Node('svg');
  p.world.append(p.setupSvg);
  p.notes = new Node('div');
  p.cardResize = { observe() {}, unobserve() {} };
  p.rules = whitelist;
  p.relayout = () => {};
  p.planeText = planeText;
  p.graph = GRAPH;
  // The panel's own code, stubbed: one element per card of the grid, in order (the open slot card a <signal-slot>).
  p.showPanel = async (container) => {
    const grid = new Node('div');
    grid.className = 'signal-cards';
    for (const card of p.panelCards) {
      const element = new Node(card.slot ? 'signal-slot' : 'section');
      element.dataset.card = card.id;
      grid.append(element);
    }
    container.append(grid);
    return null;
  };
  return p;
}

async function shown(p) {
  await p.panelSeen({ html: PANEL, plate: null, generated: false });
  await new Promise((resolve) => setTimeout(resolve, 0));
  return p;
}

for (const [form, planeText] of [
  ['the derived Stack', null],
  ['the full form', 'schema_version = 1\n[[stack]]\nname = "primary"\ninstruments = ["card:melody", "synth", "card:echo"]\n'],
  ['the slot card’s migration form', 'schema_version = 1\n[[stack]]\nname = "primary"\ninstruments = ["card:melody", "card:synth", "card:echo"]\n'],
]) {
  test(`a slot instrument (${form}): drawn in its Stack, owned there, no unstacked instance, no false edge pin`, async () => {
    const p = await shown(plane({ planeText }));
    const { model } = p;
    assert.equal(model.derived, planeText === null, model.problems.map((x) => x.message).join('; '));
    assert.deepEqual(model.plane.stacks.map((s) => [s.name, s.instruments]), [['primary', ['card:melody', 'synth', 'card:echo']]]);
    assert.equal(model.owner.get('synth'), 'synth');
    assert.equal(model.stackOf.get('synth'), 'primary');
    assert.deepEqual(model.unstacked, []);
    // The one boundary crossing is echo's output into the role endpoint: no pin on the slot.
    assert.deepEqual(
      model.pins.get('primary').map((pin) => [pin.side, pin.instrument, pin.address]),
      [['out', 'card:echo', '/echo.out']],
    );
    // Rendered: each instrument's front panel in its Stack, the slot's its open slot card.
    const s = p.stacks.get('primary');
    assert.deepEqual([...s.cards.keys()], ['card:melody', 'synth', 'card:echo']);
    assert.equal(s.cards.get('synth').localName, 'signal-slot');
    assert.equal(s.cards.get('synth').dataset.card, 'synth');
    assert.deepEqual(
      s.container.querySelector(':scope > .signal-cards').children.map((c) => c.dataset.card),
      ['melody', 'synth', 'echo'],
    );
  });
}

test('the slot listed in both forms is one instrument listed twice: an error, the derived Stack drawn', async () => {
  const p = await shown(plane({ planeText: 'schema_version = 1\n[[stack]]\nname = "primary"\ninstruments = ["card:melody", "card:synth", "synth", "card:echo"]\n' }));
  assert.equal(p.model.derived, true);
  assert.deepEqual(p.model.problems.map((x) => x.message), ['plane.toml: "synth" is listed twice (in "primary")']);
  assert.deepEqual([...p.stacks.get('primary').cards.keys()], ['card:melody', 'synth', 'card:echo']);
});

// The keys (ADR 0039, ADR 0034): the plane's capture listener, then the page's.

/** A transport to toggle, as ui/controls/keyboard.test.js's. */
function transportConnection(base = 1) {
  return {
    status: 'open',
    commits: [],
    description: {
      playing: true,
      instances: [{ address: '/transport/playhead', module: 'std.playhead', parameters: [{ name: 'run', address: '/transport/playhead.run' }] }],
      controls: [{ address: '/transport.run', ports: ['/transport/playhead.run'], base, min: 0, max: 1, default: 1, modulated: false }],
    },
    commit(address, value) {
      this.commits.push([address, value]);
      return Promise.resolve({ type: 'committed' });
    },
  };
}

/** The page's keys on a stub document (ui/controls/keyboard.js `install`): its bubbling keydown listener. */
function pageKeys() {
  const listeners = [];
  const doc = {
    addEventListener: (type, fn, options) => listeners.push({ type, fn, capture: options === true || options?.capture }),
    defaultView: { innerWidth: 1000, innerHeight: 800, getComputedStyle: () => ({ overflowY: 'visible' }), matchMedia: () => ({ matches: true }), dispatchEvent: () => true },
    scrollingElement: null,
  };
  doc.defaultView.top = doc.defaultView;
  doc.defaultView.document = doc;
  install(doc);
  return { doc, onKey: listeners.find((l) => l.type === 'keydown' && !l.capture).fn };
}

/** A keydown at `path[0]`, dispatched as the browser does: the plane's capture listener first, then the page's as it bubbles. */
function press(p, page, key, path, more = {}) {
  const event = {
    key,
    code: key === ' ' ? 'Space' : key,
    currentTarget: page.doc,
    target: path[0],
    composedPath: () => path,
    defaultPrevented: false,
    preventDefault() {
      this.defaultPrevented = true;
    },
    stopPropagation() {},
    ...more,
  };
  p.key(event);
  page.onKey(event);
  return event;
}

function focusedPlane() {
  const p = new SignalPlane();
  p.deep = new Node('div');
  p.stage = new Node('div');
  p.viewport = { w: 800, h: 600 };
  p.view = { k: 1, x: 0, y: 0 };
  p.views = [];
  p.setView = (view) => p.views.push(view);
  p.focusRect = () => ({ x: 0, y: 0, w: 800, h: 4000 });
  return p;
}

test('Space with the focus on the plane plays and stops the transport at Stack, Wiring and Control; fields, buttons and controls keep it', () => {
  const page = pageKeys();
  const connection = transportConnection(1);
  setDefaultConnection(connection);
  const p = focusedPlane();
  for (const level of ['stack', 'wiring', 'control']) {
    p.level = () => level;
    const event = press(p, page, ' ', [p]);
    assert.equal(event.defaultPrevented, true, `${level}: taken by the page's transport command`);
  }
  assert.deepEqual(connection.commits, [['/transport.run', 0], ['/transport.run', 1], ['/transport.run', 0]], 'each press toggled it, as a click on its toggle writes');
  assert.deepEqual(p.views, [], 'Space no longer scrolls the Stack');
  // A text field, a native button, and a focused control that takes Space keep it.
  p.level = () => 'stack';
  press(p, page, ' ', [{ nodeType: 1, localName: 'input', type: 'text' }, p]);
  press(p, page, ' ', [{ nodeType: 1, localName: 'button' }, p]);
  press(p, page, ' ', [{ nodeType: 1, localName: 'signal-toggle' }, p], { defaultPrevented: true });
  assert.equal(connection.commits.length, 3);
  setDefaultConnection(null);
});

test('the plane’s keys are commands of the shared keymap, rebindable through keys.toml', () => {
  const page = pageKeys();
  const p = focusedPlane();
  p.level = () => 'stack';
  const travelled = [];
  p.travelTo = (z) => travelled.push(z);
  p.fitSetup = () => travelled.push('setup');
  assert.equal(press(p, page, 'ArrowDown', [p]).defaultPrevented, true);
  assert.equal(press(p, page, 'PageDown', [p]).defaultPrevented, true);
  assert.equal(p.views.length, 2, 'the Stack scrolled a little, then a page');
  press(p, page, 'Enter', [p]);
  press(p, page, 'Escape', [p]);
  assert.equal(travelled.length, 2, 'in to Wiring, out to the Setup');
  assert.equal(travelled.at(-1), 'setup');
  // keys.toml moves plane.out to Q: Escape is no plane command there any more.
  applyKeys({ overrides: { 'plane.out': ['Q'] } });
  try {
    press(p, page, 'Escape', [p]);
    assert.equal(travelled.length, 2);
    press(p, page, 'q', [p]);
    assert.equal(travelled.length, 3);
  } finally {
    applyKeys({ overrides: {} });
  }
});

