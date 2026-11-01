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
  get tagName() {
    return this.localName.toUpperCase();
  }
  get id() {
    return this.attributes.get('id') ?? '';
  }
  matches(selector) {
    const list = selector.split(',').map((x) => x.trim());
    if (list.length > 1) return list.some((x) => this.matches(x));
    const child = /^(.*\S)\s*>\s*(\S+)$/.exec(selector);
    if (child) return this.matches(child[2]) && Boolean(this.parentElement?.matches(child[1]));
    if (selector.startsWith('.')) return this.classList.contains(selector.slice(1));
    if (selector === '[bind]') return this.hasAttribute('bind');
    return this.localName === selector;
  }
  contains(node) {
    for (let at = node; at; at = at.parentElement) if (at === this) return true;
    return false;
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

/**
 * A test with the stub DOM as `document` for its length only: `node --test
 * ui/` runs every test file in this one process (ui/index.js), and the
 * others run without a document.
 */
function domTest(name, fn) {
  test(name, async () => {
    const saved = { document: globalThis.document, frame: globalThis.requestAnimationFrame };
    globalThis.document = { createElement: (tag) => new Node(tag), createElementNS: (_ns, tag) => new Node(tag) };
    globalThis.requestAnimationFrame = (callback) => setTimeout(callback, 0);
    try {
      await fn();
    } finally {
      for (const [key, value] of [['document', saved.document], ['requestAnimationFrame', saved.frame]]) {
        if (value === undefined) delete globalThis[key];
        else globalThis[key] = value;
      }
    }
  });
}

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
      if (card.slot) element.setAttribute('bind', `/${card.slot}`);
      else {
        element.className = 'signal-card';
        const heading = new Node('h2');
        heading.textContent = card.heading;
        const knob = new Node('signal-knob');
        knob.setAttribute('bind', card.binds[0]);
        knob.binding = { address: card.binds[0] };
        element.append(heading, knob);
      }
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
  domTest(`a slot instrument (${form}): drawn in its Stack, owned there, no unstacked instance, no false edge pin`, async () => {
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

domTest('the slot listed in both forms is one instrument listed twice: an error, the derived Stack drawn', async () => {
  const p = await shown(plane({ planeText: 'schema_version = 1\n[[stack]]\nname = "primary"\ninstruments = ["card:melody", "card:synth", "synth", "card:echo"]\n' }));
  assert.equal(p.model.derived, true);
  assert.deepEqual(p.model.problems.map((x) => x.message), ['plane.toml: "synth" is listed twice (in "primary")']);
  assert.deepEqual([...p.stacks.get('primary').cards.keys()], ['card:melody', 'synth', 'card:echo']);
});

// The Setup level follows a Setup change without a new generation (review of 2026-11-01).

const config = (channels) => ({ channels, min_rate: 44100, max_rate: 192000, buffer: [15, 4096], sample_format: 'f32' });
const SSL_ID = 'coreaudio:AppleUSBAudioEngine:Solid State Logic:SSL 2+ MKII';
const SPEAKERS_ID = 'coreaudio:BuiltInSpeakerDevice';
const INVENTORY = [
  { index: 0, name: 'MacBook Pro Speakers', id: SPEAKERS_ID, direction: 'output', default: true, rate: 48000, configs: [config(2)] },
  { index: 1, name: 'SSL 2+ MKII', id: SSL_ID, direction: 'output', default: false, rate: 48000, configs: [config(4)] },
];

/** A connection answering the plane's reads from `state` (its Setup, its inventory's resolution), recording each request; `emit` pushes a message. */
function setupConnection(state) {
  const handlers = new Map();
  return {
    status: 'open',
    description: null,
    requests: [],
    on(type, fn) {
      if (!handlers.has(type)) handlers.set(type, new Set());
      handlers.get(type).add(fn);
      return () => handlers.get(type).delete(fn);
    },
    emit(type, message) {
      for (const fn of handlers.get(type) ?? []) fn(message);
    },
    request(message) {
      this.requests.push(message);
      if (message.type === 'panel.read') return Promise.resolve({ exists: true, html: PANEL, plate: null, text: '' });
      if (message.type === 'graph') return Promise.resolve(GRAPH);
      if (message.type === 'setup.get') return Promise.resolve({ type: 'setup', setup: state.setup, local: null, playing: true });
      if (message.type === 'setup.devices') return Promise.resolve({ type: 'setup.devices', devices: INVENTORY, output: { found: state.found } });
      return Promise.reject(new Error(`unexpected ${message.type}`));
    },
  };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 5));

domTest('after a Setup change on the same generation the Setup level’s cable agrees with the active Setup: output device, then output channels', async () => {
  const state = { setup: { output: { name: 'SSL 2+ MKII', id: SSL_ID }, outputChannels: null }, found: 1 };
  const connection = setupConnection(state);
  const p = plane();
  p.graph = null;
  p.attach(connection);
  await settle();
  const cable = () => p.model.bindings.map((b) => [b.device, b.channels]);
  assert.deepEqual(cable(), [[`id:${SSL_ID}`, [1, 2]]], 'main on the SSL 2+’s Out 1–2');
  // The Setup tab applies another output device (`setup.apply`'s reply, on this connection); no `app` generation comes.
  state.setup = { output: { name: 'MacBook Pro Speakers', id: SPEAKERS_ID }, outputChannels: null };
  state.found = 0;
  connection.emit('message', { type: 'applied', saved: true, restarted: false, setup: state.setup, id: 7 });
  await settle();
  assert.deepEqual(cable(), [[`id:${SPEAKERS_ID}`, [1, 2]]], 'the cable moved to the speakers');
  // A cable dragged to Out 3–4 (`device.route`'s reply is `applied` too).
  state.setup = { output: { name: 'SSL 2+ MKII', id: SSL_ID }, outputChannels: [3, 4] };
  state.found = 1;
  connection.emit('message', { type: 'applied', saved: true, restarted: false, setup: state.setup, routed: { role: 'main', scope: 'machine', notes: [] }, id: 8 });
  await settle();
  assert.deepEqual(cable(), [[`id:${SSL_ID}`, [3, 4]]], 'on Out 3–4');
  // A refused apply changes nothing and reads nothing.
  const reads = connection.requests.filter((m) => m.type === 'setup.get').length;
  connection.emit('message', { type: 'applied', saved: false, restarted: false, error: 'disk full', id: 9 });
  await settle();
  assert.equal(connection.requests.filter((m) => m.type === 'setup.get').length, reads);
  // The hardware is never enumerated again for it: no `refresh`.
  assert.equal(connection.requests.filter((m) => m.type === 'setup.devices').length, 3);
  assert.ok(connection.requests.every((m) => !m.refresh));
  p.detach();
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

/**
 * A keydown at `path[0]`, dispatched as the browser does: the plane's
 * capture listener first, then the target's own (`atTarget`, a focused
 * control's keydown), then the page's as it bubbles.
 */
function press(p, page, key, path, more = {}, atTarget = () => {}) {
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
  atTarget(event);
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

domTest('Space with the focus on the plane plays and stops the transport at Stack, Wiring and Control; fields, buttons and controls keep it', () => {
  const page = pageKeys();
  const connection = transportConnection(1);
  setDefaultConnection(connection);
  try {
    const p = focusedPlane();
    for (const level of ['stack', 'wiring', 'control']) {
      p.level = () => level;
      const event = press(p, page, ' ', [p]);
      assert.equal(event.defaultPrevented, true, `${level}: taken by the page's transport command`);
    }
    assert.deepEqual(connection.commits, [['/transport.run', 0], ['/transport.run', 1], ['/transport.run', 0]], 'each press toggled it, as a click on its toggle writes');
    assert.deepEqual(p.views, [], 'Space no longer scrolls the Stack');
    // A text field and a native button keep it.
    p.level = () => 'stack';
    press(p, page, ' ', [{ nodeType: 1, localName: 'input', type: 'text' }, p]);
    press(p, page, ' ', [{ nodeType: 1, localName: 'button' }, p]);
    // A toggle in a front panel: the plane's capture leaves Space alone, the toggle's own keydown takes it, the page then does not.
    let flipped = 0;
    press(p, page, ' ', [{ nodeType: 1, localName: 'signal-toggle' }, p], {}, (event) => {
      flipped++;
      event.preventDefault();
    });
    assert.equal(flipped, 1);
    assert.equal(connection.commits.length, 3);
  } finally {
    setDefaultConnection(null);
  }
});

domTest('with Edit layout over the plane its Escape closes it and the plane underneath takes no key', () => {
  const page = pageKeys();
  const p = focusedPlane();
  p.level = () => 'stack';
  p.fitSetup = () => p.views.push('setup');
  p.editLayout();
  assert.ok(p.editing);
  assert.equal(press(p, page, 'ArrowDown', [p]).defaultPrevented, false);
  press(p, page, 'Escape', [p]);
  assert.equal(p.editing, null, 'closed');
  assert.deepEqual(p.views, [], 'nothing scrolled, the Setup not shown');
});

domTest('the plane’s keys are commands of the shared keymap, rebindable through keys.toml', () => {
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

// Edit layout and card moves from the main window (review of 2026-11-01; ADR 0051 §6, ADR 0032).

domTest('at the Stack level the bar has Edit layout: it opens the panel editor over the plane, Done closes it', async () => {
  const p = await shown(plane());
  p.bar = new Node('nav');
  p.stage = new Node('div');
  p.renderBar('setup', 0.5);
  assert.equal(p.bar.querySelector('.edit-layout'), null, 'not at the Setup level');
  p.renderBar('stack', 1);
  const edit = p.bar.querySelector('.edit-layout');
  assert.equal(edit.textContent, 'Edit layout');
  edit.listeners.find(([type]) => type === 'click')[1]();
  const overlay = p.stage.querySelector('.editor');
  assert.ok(overlay, 'the editor over the stage');
  // ADR 0053: the editor of ui/main.html in this page, in a shadow root with the panel's style sheets; never a frame of the panel's page.
  const body = overlay.querySelector('.editor-body');
  const panel = body.shadowRoot.querySelector('signal-panel');
  assert.equal(panel.getAttribute('file'), 'main.html');
  assert.equal(typeof panel.attributeValue, 'function', 'its relative URLs resolved from the panel’s page');
  assert.ok(body.shadowRoot.querySelectorAll('link').length >= 1, 'the panel’s style sheets');
  const everything = [p.stage, ...p.stage.all(), ...p.stage.all().flatMap((n) => (n.shadowRoot ? [n.shadowRoot, ...n.shadowRoot.all()] : []))];
  assert.deepEqual(everything.filter((n) => ['iframe', 'frame', 'object', 'embed'].includes(n.localName)), [], 'no frame');
  assert.ok(!everything.some((n) => /\/project\//.test(String(n.src ?? n.getAttribute?.('src') ?? ''))), 'no project page loaded');
  p.editLayout();
  assert.equal(p.stage.querySelectorAll('.editor').length, 1, 'one at a time');
  overlay.querySelector('button').listeners.find(([type]) => type === 'click')[1]();
  assert.equal(p.stage.querySelector('.editor'), null, 'Done closes it');
  assert.equal(p.editing, null);
});

domTest('a front panel’s context menu moves its card to a new panel window; a card another window holds is a placeholder with Show and Bring back', async () => {
  const p = await shown(plane());
  p.stage = new Node('div');
  p.level = () => 'stack';
  const posted = [];
  const s = p.stacks.get('primary');
  const melody = s.cards.get('card:melody');
  const echo = s.cards.get('card:echo');
  const at = (...path) => ({ composedPath: () => [...path, s.element, p] });
  // In a browser: Edit layout only.
  assert.deepEqual(p.contextMenu(at(melody)).items.map((i) => i.id), ['edit-layout']);
  // In Signal.app's main window (`connectPlaneCards` gave the channel).
  p.cardChannel = (message) => posted.push(message);
  const menu = p.contextMenu(at(melody));
  assert.deepEqual(menu.items.map((i) => i.id), ['new', 'edit-layout']);
  menu.act('new');
  assert.deepEqual(posted, [{ type: 'card.move', card: 'melody', to: 'new', size: { width: 184, height: 192 } }], 'the card by the app’s id, the window its size');
  // A control bound to a parameter keeps its own menu.
  assert.equal(p.contextMenu(at(melody.querySelector('signal-knob'), melody)), null);
  // The app says the echo card is held by a panel window, which is a target now.
  p.windowState({ view: 'main', held: [{ card: 'echo', where: 'Panel window 1' }], targets: [{ index: 1, label: 'Panel window 1' }] });
  assert.equal(echo.hasAttribute('data-held'), true);
  assert.equal(melody.hasAttribute('data-held'), false);
  const note = echo.querySelector('.held-note');
  assert.equal(note.querySelector('span').textContent, 'Echo — in Panel window 1');
  const [show, back] = note.querySelectorAll('button');
  show.listeners.find(([type]) => type === 'click')[1]();
  back.listeners.find(([type]) => type === 'click')[1]();
  assert.deepEqual(posted.slice(1), [{ type: 'window.show', card: 'echo' }, { type: 'card.move', card: 'echo', to: 'here' }]);
  assert.deepEqual(p.contextMenu(at(melody)).items.map((i) => i.label), ['Move to Panel window 1', 'Move to new panel window', 'Edit layout…']);
  // The open slot card is a card of its own (`slot.<name>`, as the app names it).
  p.contextMenu(at(s.cards.get('synth'))).act('new');
  assert.equal(posted.at(-1).card, 'slot.synth');
  // Brought back: no placeholder.
  p.windowState({ view: 'main', held: [], targets: [] });
  assert.equal(echo.hasAttribute('data-held'), false);
  assert.equal(echo.querySelector('.held-note'), null);
});
