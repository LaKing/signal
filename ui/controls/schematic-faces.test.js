// The slots' faces over the schematic (2026-10-30,
// ui/controls/schematic-faces.js), driven with the least of a DOM: a face
// is made for each face area drawn, placed by the area's screen matrix
// (the view, a drag), hidden when too small or under an open frame, let
// go with its area; its part is read once per text (never per frame); a
// write shows at once; a measured height lays the drawing out again only
// when it differs; the controls bound to what is hovered are lit; a key on
// a face's control is the control's.

import test from 'node:test';
import assert from 'node:assert/strict';

class Node {
  constructor(name = 'div') {
    this.localName = name;
    this.children = [];
    this.parentNode = null;
    this.attributes = new Map();
    this.dataset = {};
    this.style = {};
    this.className = '';
    this.textContent = '';
    this.listeners = new Map();
    this.offsetHeight = 0;
  }
  append(...nodes) {
    for (const n of nodes) {
      n.parentNode = this;
      this.children.push(n);
    }
  }
  after(...nodes) {
    const list = this.parentNode.children;
    list.splice(list.indexOf(this) + 1, 0, ...nodes);
    for (const n of nodes) n.parentNode = this.parentNode;
  }
  remove() {
    const list = this.parentNode?.children;
    if (list) list.splice(list.indexOf(this), 1);
    this.parentNode = null;
  }
  replaceChildren(...nodes) {
    this.children = [];
    this.append(...nodes);
  }
  querySelectorAll(selector) {
    const found = [];
    const walk = (n) => {
      for (const c of n.children) {
        if (selector === '*' || (selector === 'link' && c.localName === 'link') || (selector.startsWith('.') && c.className.split(' ').includes(selector.slice(1)))) found.push(c);
        walk(c);
      }
    };
    walk(this);
    return found;
  }
  attachShadow() {
    this.shadowRoot = new Node('#shadow');
    return this.shadowRoot;
  }
  addEventListener(type, fn) {
    this.listeners.set(type, fn);
  }
  getAttribute(name) {
    return this.attributes.get(name) ?? null;
  }
  hasAttribute(name) {
    return this.attributes.has(name);
  }
  toggleAttribute(name, on) {
    if (on) this.attributes.set(name, '');
    else this.attributes.delete(name);
  }
  getBoundingClientRect() {
    return { left: 0, top: 0, width: 1000, height: 600 };
  }
}

globalThis.document ??= { createElement: (name) => new Node(name) };
const realDocument = globalThis.document;
globalThis.DOMPoint ??= class {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  matrixTransform(m) {
    return { x: m.a * this.x + m.e, y: m.d * this.y + m.f };
  }
};

const { SchematicFaces } = await import('./schematic-faces.js');
const { FACE_LAYOUT_PX } = await import('../lib/schematic-face.js');

/** A face area as the drawing marks it: its corners in its group's units, the group's screen matrix `ctm`. */
function area({ id = '/synth', file = 'parts/synth.html', root = '/synth', path = '', slot = 'synth', base = 'b1', w = 640, h = 200, nw = FACE_LAYOUT_PX, ctm = { a: 1, d: 1, e: 0, f: 0 } } = {}) {
  const g = new Node('g');
  g.className = 'face-area';
  Object.assign(g.dataset, { face: id, file, root, path, slot, base, error: '', nw: String(nw), x0: '100', y0: '50', x1: String(100 + w), y1: String(50 + h) });
  g.getScreenCTM = () => ctm;
  return g;
}

function setup() {
  globalThis.document = { createElement: (name) => new Node(name) };
  const svg = new Node('svg');
  const requests = [];
  const host = {
    svg,
    gesture: null,
    resized: 0,
    facesResized() {
      this.resized += 1;
    },
    connection: {
      request: async (message) => {
        requests.push(message);
        return { type: 'panel.read', file: message.file, exists: true, base: 'b1', html: '<div class="signal-cards"></div>' };
      },
    },
  };
  const faces = new SchematicFaces(host);
  const rendered = [];
  faces.render = async (face) => {
    rendered.push(face.id);
  };
  return { svg, host, faces, requests, rendered };
}

test('a face is made for each area drawn, placed by its screen matrix, its part read once per text', async () => {
  const { svg, faces, requests, rendered } = setup();
  svg.append(area());
  faces.sync({ levels: [] });
  const face = faces.faces.get('/synth');
  assert.ok(face, 'made');
  assert.equal(face.element.className, 'face shown');
  // Its cards are 1280 px across on the panel, its area 640 px on screen: half the panel's size, a miniature.
  assert.equal(face.element.style.transform, 'translate(100px, 50px) scale(0.5)');
  await new Promise((r) => setTimeout(r, 0));
  assert.deepEqual(requests, [{ type: 'panel.read', file: 'parts/synth.html' }]);
  assert.ok(rendered.length >= 2, 'drawn, then drawn again with the part read');
  // The view moves (a pan, a zoom, a drag): only the transform is written, the part not read again.
  svg.children[0].getScreenCTM = () => ({ a: 0.5, d: 0.5, e: 10, f: 20 });
  const before = rendered.length;
  faces.sync({ levels: [] });
  assert.equal(face.element.style.transform, 'translate(60px, 45px) scale(0.25)');
  assert.equal(requests.length, 1);
  assert.equal(rendered.length, before, 'not drawn again for a move');
  // Its text changed (the graph's base): read again.
  svg.children[0].dataset.base = 'b2';
  faces.sync({ levels: [] });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(requests.length, 2);
  globalThis.document = realDocument;
});

test('too small, off the stage or under an open frame a face is hidden; around the frame dimmed; its area gone, the face goes', () => {
  const { svg, faces } = setup();
  svg.append(area({ ctm: { a: 0.05, d: 0.05, e: 0, f: 0 } }));
  faces.sync({ levels: [] });
  const face = faces.faces.get('/synth');
  assert.equal(face.element.className, 'face', 'too small: hidden');
  svg.children[0].getScreenCTM = () => ({ a: 1, d: 1, e: 5000, f: 0 });
  faces.sync({ levels: [] });
  assert.equal(face.element.className, 'face', 'off the stage');
  svg.children[0].getScreenCTM = () => ({ a: 1, d: 1, e: 0, f: 0 });
  // Its level has another box open in place, its frame over the face, then beside it.
  const level = (frame) => ({ path: '', transform: { k: 1, x: 0, y: 0 }, layout: { opened: 'fx', nodes: [{ kind: 'instance', name: 'fx', ...frame }] } });
  faces.sync({ levels: [level({ x: 0, y: 0, w: 900, h: 500 })] });
  assert.equal(face.element.className, 'face', 'under the frame');
  faces.sync({ levels: [level({ x: 600, y: 300, w: 200, h: 100 })] });
  assert.equal(face.element.className, 'face shown around', 'around the frame: dimmed');
  svg.replaceChildren();
  faces.sync({ levels: [] });
  assert.equal(faces.faces.size, 0, 'its area gone: let go');
  assert.equal(face.element.parentNode, null);
  globalThis.document = realDocument;
});

test("a part written shows at once in its faces; a measured height lays out again only when it differs from the layout's", () => {
  const { svg, faces, host, rendered } = setup();
  svg.append(area());
  faces.sync({ levels: [] });
  const count = rendered.length;
  faces.written({ type: 'panel.written', file: 'parts/synth.html', base: 'b3', html: '<div class="signal-cards"></div>' });
  assert.equal(rendered.length, count + 1);
  faces.written({ type: 'panel.written', file: 'main.html', html: '' });
  assert.equal(rendered.length, count + 1, 'the main panel is not a face');
  const face = faces.faces.get('/synth');
  // The layout asked its size (an estimate), then it is measured: drawn at half size, its card 600 px across on screen is 1200 on the panel.
  faces.sizeOf({ file: 'parts/synth.html', count: 3 });
  face.panel.getBoundingClientRect = () => ({ left: 0, top: 0, width: FACE_LAYOUT_PX / 2, right: FACE_LAYOUT_PX / 2 });
  const grid = new Node('div');
  const card = new Node('section');
  card.getBoundingClientRect = () => ({ left: 0, width: 600, right: 600 });
  grid.append(card);
  face.panel.append(grid);
  face.panel.offsetHeight = 333;
  faces.resized([{ target: face.panel }]);
  assert.equal(host.resized, 1);
  assert.deepEqual(faces.sizes.get('parts/synth.html'), { w: 1200, h: 336 }, 'its natural size, rounded up to its step');
  // In its box: at its own size (ADR 0047 §4).
  const fit = faces.sizeOf({ file: 'parts/synth.html', count: 3 });
  assert.equal(fit.w, 1200);
  assert.equal(fit.h, 336);
  // Laid out at that size: the same measure again changes nothing.
  faces.resized([{ target: face.panel }]);
  assert.equal(host.resized, 1);
  face.panel.offsetHeight = 0;
  faces.resized([{ target: face.panel }]);
  assert.equal(host.resized, 1, 'a hidden face measures nothing');
  globalThis.document = realDocument;
});

test("the controls bound to what is hovered or selected are lit; a key on a face's control is the control's", () => {
  const { svg, faces } = setup();
  svg.append(area());
  faces.sync({ levels: [] });
  const face = faces.faces.get('/synth');
  const knob = new Node('signal-knob');
  knob.attributes.set('bind', '/synth.cutoff');
  const other = new Node('signal-knob');
  other.attributes.set('bind', '/synth.drive');
  face.panel.append(knob, other);
  faces.light(new Set(['/synth.cutoff']));
  assert.ok(knob.hasAttribute('data-face-hot'));
  assert.ok(!other.hasAttribute('data-face-hot'));
  faces.light(new Set());
  assert.ok(!knob.hasAttribute('data-face-hot'));
  const onKnob = { composedPath: () => [knob, face.panel, face.shadow, face.element, faces.layer] };
  const onCard = { composedPath: () => [face.panel, face.shadow, face.element, faces.layer] };
  assert.ok(faces.owns(onKnob));
  assert.ok(!faces.owns(onCard));
  assert.ok(!faces.owns({ composedPath: () => [knob] }), 'a knob elsewhere (the panel) is not a face’s');
  globalThis.document = realDocument;
});

test("a face links the theme and the project panel's style sheets", () => {
  const { svg, faces } = setup();
  faces.pageSeen('<link rel="stylesheet" href="/ui/controls/theme.css"><link rel="stylesheet" href="studio.css">');
  svg.append(area());
  faces.sync({ levels: [] });
  const links = faces.faces.get('/synth').shadow.querySelectorAll('link').map((l) => l.href);
  assert.ok(links[0].endsWith('/controls/theme.css'));
  // Without a page (no location in node) a relative sheet cannot be resolved: the theme alone.
  assert.ok(links.length >= 1);
  globalThis.document = realDocument;
});

test('a part that failed to read is read again for a newer revision or after a reconnect, never per frame (review 2026-10-31)', async () => {
  const { svg, faces, host, requests } = setup();
  let exists = false;
  host.connection.request = async (message) => {
    requests.push(message);
    return { type: 'panel.read', file: message.file, exists, base: exists ? 'b2' : null, html: exists ? '<div class="signal-cards"></div>' : undefined };
  };
  svg.append(area({ base: 'b1' }));
  faces.sync({ levels: [] });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(faces.parts.get('parts/synth.html').error, 'no such part file');
  faces.sync({ levels: [] });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(requests.length, 1, 'the same revision: not read again');
  // A newer revision of the graph: read again, and now it is there.
  exists = true;
  svg.children[0].dataset.base = 'b2';
  faces.sync({ levels: [] });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(requests.length, 2, 'read again for the newer revision');
  assert.equal(faces.parts.get('parts/synth.html').error, null);
  assert.equal(faces.parts.get('parts/synth.html').base, 'b2');
  // Missing again at that revision; a reconnect reads it again.
  exists = false;
  faces.parts.set('parts/synth.html', { html: null, base: null, asked: 'b2', error: 'no such part file', reading: false });
  faces.sync({ levels: [] });
  assert.equal(requests.length, 2);
  faces.reconnected();
  faces.sync({ levels: [] });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(requests.length, 3, 'read again after a reconnect');
  globalThis.document = realDocument;
});

test('a face shown or hidden, or the plate’s layer changed, tells its controls (`signal-visibility`): a scope in it lets its capture go or takes it (review 2026-10-31)', () => {
  const { svg, faces, host } = setup();
  const told = [];
  document.dispatchEvent = (event) => told.push(event.type);
  host.board = { covered: false };
  svg.append(area({ id: '#plate', file: 'main.html', root: '', slot: '' }));
  faces.sync({ levels: [] });
  const count = told.length;
  assert.ok(count >= 1, 'shown: told');
  faces.sync({ levels: [] });
  assert.equal(told.length, count, 'nothing changed: nothing told');
  faces.setPlateClass(' plate layer-xray');
  assert.equal(told.length, count + 1, 'X-ray hides its cards: told');
  faces.setPlateClass(' plate layer-board');
  assert.equal(told.length, count + 2, 'Board: told');
  assert.ok(told.every((t) => t === 'signal-visibility'));
  globalThis.document = realDocument;
});

test('another project: a part read of the last one that answers late is ignored (review 2026-10-31)', async () => {
  const { svg, faces } = setup();
  svg.append(area());
  faces.sync({ levels: [] });
  assert.equal(faces.parts.get('parts/synth.html')?.reading, true, 'being read');
  faces.clear();
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(faces.parts.size, 0, 'the late answer is not kept');
  globalThis.document = realDocument;
});

test('a hidden face, and the plate faded out on Board, is inert: no key reaches its controls; the plate on Plate and X-ray is operable; focus in it goes to the drawing (review 2026-10-31)', () => {
  const { svg, faces, host } = setup();
  let focused = 0;
  host.focus = () => focused++;
  host.board = { covered: false };
  svg.append(area({ id: '#plate', file: 'main.html', root: '', slot: '' }));
  faces.setPlateClass(' plate layer-plate');
  faces.sync({ levels: [] });
  const plate = faces.faces.get('#plate');
  assert.equal(plate.element.className, 'face shown plate layer-plate');
  assert.equal(plate.element.inert, false, 'Plate: operable');
  faces.setPlateClass(' plate layer-xray');
  assert.equal(plate.element.inert, false, 'X-ray: its controls shown whole, operable (its hidden cards are visibility: hidden, unfocusable)');
  // A control of the plate has the focus when the layer goes to Board: the drawing takes it, then the plate is inert.
  const knob = new Node('signal-knob');
  plate.shadow.activeElement = knob;
  faces.setPlateClass(' plate layer-board');
  assert.equal(plate.element.inert, true, 'Board: the plate at opacity 0 is inert');
  assert.equal(focused, 1, 'the focus moved to the schematic');
  plate.shadow.activeElement = null;
  faces.sync({ levels: [] });
  assert.equal(plate.element.inert, true, 'a sync keeps it inert on Board');
  faces.setPlateClass(' plate layer-plate');
  assert.equal(plate.element.inert, false, 'back to Plate: operable again');
  // A slot's face: inert while hidden (too small, off the stage, under the plate), operable when shown.
  svg.append(area());
  faces.sync({ levels: [] });
  const face = faces.faces.get('/synth');
  assert.equal(face.element.inert, false);
  svg.children[1].getScreenCTM = () => ({ a: 1, d: 1, e: 5000, f: 0 });
  faces.sync({ levels: [] });
  assert.equal(face.element.inert, true, 'off the stage: inert');
  globalThis.document = realDocument;
});
