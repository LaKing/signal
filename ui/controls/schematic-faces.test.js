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
const { FACE_W } = await import('../lib/schematic-face.js');

/** A face area as the drawing marks it: its corners in its group's units, the group's screen matrix `ctm`. */
function area({ id = '/synth', file = 'parts/synth.html', root = '/synth', path = '', slot = 'synth', base = 'b1', w = FACE_W, h = 200, ctm = { a: 1, d: 1, e: 0, f: 0 } } = {}) {
  const g = new Node('g');
  g.className = 'face-area';
  Object.assign(g.dataset, { face: id, file, root, path, slot, base, error: '', x0: '100', y0: '50', x1: String(100 + w), y1: String(50 + h) });
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
  assert.equal(face.element.style.transform, 'translate(100px, 50px) scale(1)');
  await new Promise((r) => setTimeout(r, 0));
  assert.deepEqual(requests, [{ type: 'panel.read', file: 'parts/synth.html' }]);
  assert.ok(rendered.length >= 2, 'drawn, then drawn again with the part read');
  // The view moves (a pan, a zoom, a drag): only the transform is written, the part not read again.
  svg.children[0].getScreenCTM = () => ({ a: 0.5, d: 0.5, e: 10, f: 20 });
  const before = rendered.length;
  faces.sync({ levels: [] });
  assert.equal(face.element.style.transform, 'translate(60px, 45px) scale(0.5)');
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
  svg.append(area({ ctm: { a: 0.1, d: 0.1, e: 0, f: 0 } }));
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
  // The layout asked its size (an estimate), then it is measured.
  faces.sizeOf({ file: 'parts/synth.html', count: 3 });
  face.panel.offsetHeight = 333;
  faces.resized([{ target: face.panel }]);
  assert.equal(host.resized, 1);
  assert.equal(faces.sizes.get('parts/synth.html').h, 336, 'rounded up to its step');
  // Laid out at that size: the same measure again changes nothing.
  assert.equal(faces.sizeOf({ file: 'parts/synth.html', count: 3 }).h, 336);
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
