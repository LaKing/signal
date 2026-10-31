// <signal-schematic>'s deep zoom (ADR 0047 §8, phase 3; the owner: "a
// click should zoom into the chip … the structure around it fades … the
// chip's inner schematic appears", replacing 2026-10-22's opening in place
// and its ×), driven through its real pointer, key, zoom and render code
// with the least of a DOM it needs: the drawing itself (`drawLevel` and the
// marks) is left out, the scene is the real one. Every level is a board
// (the top level's, each inside a die drawn in its package), as the page
// lays them out. Without `requestAnimationFrame` (node) a travel arrives at
// once, as with reduced motion.

import test from 'node:test';
import assert from 'node:assert/strict';
import { layout } from '../lib/schematic-layout.js';
import { DIE_MAX_K, FULL_K, HIT_OPACITY, belowThreshold, dieOpacity } from '../lib/board-zoom.js';
import { fit, toScreen, zoomAt } from '../lib/schematic-view.js';
import { KICK } from '../lib/schematic-scene.test.js';
import { hitTest } from '../lib/schematic-hit.js';
import { wireNet } from '../lib/board-net.js';
import { bake } from '../lib/schematic-draw.js';
import { DragFollow } from '../lib/schematic-drag.js';

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };

const { SignalSchematic } = await import('./schematic.js');

const viewport = { w: 1000, h: 600 };

/** KICK's levels as boards (each inside a die, its package widened towards it), and a library composite `/lib` (its inside not in the graph) beside them. */
function kickLayouts(counter = null) {
  const top = { ...KICK.top, instances: [...KICK.top.instances, { name: 'lib', module: 'lib.voice', composite: true, ports: [], parameters: [] }] };
  const layouts = new Map();
  const of = (level) => {
    if (!layouts.has(level)) {
      if (counter) counter.laid++;
      const dieOf = (node) => of(node.module);
      layouts.set(level, level === '' ? layout(top, {}, { board: { dieOf } }) : KICK.composites[level] ? layout(KICK.composites[level], {}, { board: { die: true, dieOf } }) : null);
    }
    return layouts.get(level);
  };
  return of;
}

/** A schematic showing KICK fitted (as on loading), every redraw its real `render` without the drawing. */
function schematic(counter = null) {
  const s = Object.create(SignalSchematic.prototype);
  const layoutOf = kickLayouts(counter);
  Object.assign(s, {
    graph: { project: 'kick' },
    viewport,
    view: fit(layoutOf('').bounds, viewport),
    fitted: true,
    open: new Set(),
    pinned: new Set(),
    zooming: null,
    zoomedOut: false,
    selected: [],
    gesture: null,
    tracing: null,
    groups: new Map(),
    order: [],
    editor: { editing: false, placing: false, starts: () => false, key: () => false },
    menu: { list: {} },
    svg: { getBoundingClientRect: () => ({ left: 0, top: 0 }), setPointerCapture() {}, classList: { toggle() {} }, replaceChildren() {} },
  });
  s.layoutOf = layoutOf;
  s.select = (parts) => (s.selected = parts);
  s.ended = 0;
  s.endTrace = () => {
    s.tracing = null;
    s.ended++;
  };
  for (const name of ['focus', 'hover', 'say', 'closeMenu', 'collectBars', 'applyMarks', 'applyTrace', 'applyDrag', 'drawCrumbs', 'showSide', 'applyBars', 'updateLevels']) s[name] = () => {};
  s.drawLevel = (level) => {
    if (!s.groups.has(level.path)) s.groups.set(level.path, { g: {}, nodeCache: new Map() });
    return false;
  };
  s.sideStateKey = () => '';
  s.schedule = () => s.render();
  s.render();
  return s;
}

let time = 0;
/** A press and release at `at` (moved to `to` between, when given), well after any earlier click. */
function click(s, at, to = null, gap = 1000) {
  time += gap;
  const event = (p) => ({ button: 0, pointerId: 1, clientX: p.x, clientY: p.y, shiftKey: false, metaKey: false, ctrlKey: false, timeStamp: time });
  s.pointerDown(event(at));
  if (to) s.pointerMove(event(to));
  s.pointerUp(event(to ?? at));
}

function key(s, name) {
  s.key({ key: name, composedPath: () => [s], preventDefault() {}, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false });
}

/** Instance `name`'s box at level `path` on screen. */
function rectOf(s, path, name) {
  const level = s.drawn.levels.find((l) => l.path === path);
  const node = level.stored.nodes.find((n) => n.name === name);
  const at = toScreen(level.transform, node);
  return { ...at, w: node.w * level.transform.k, h: node.h * level.transform.k };
}

/** The middle of instance `name`'s box at level `path` on screen. */
function middleOf(s, path, name) {
  const r = rectOf(s, path, name);
  return { x: r.x + r.w / 2, y: r.y + r.h / 2 };
}

/** The scale of the top level on screen. */
const topK = (s) => s.drawn.levels[0].transform.k;

/** The boxes of a level as stored: what must never move. */
const boxes = (s, path) => s.layoutOf(path === '' ? '' : s.drawn.levels.find((l) => l.path === path).level).nodes.map((n) => [n.id, n.x, n.y, n.w, n.h]);

test('a click on a chip zooms into it: its package fills the stage, its die drawn whole in it, nothing moved, the surroundings faded', () => {
  const s = schematic();
  const before = boxes(s, '');
  click(s, middleOf(s, '', 'drums'));
  assert.deepEqual([...s.pinned], ['/drums']);
  assert.deepEqual([...s.drawn.open], ['/drums']);
  assert.deepEqual(s.selected.map((p) => p.address), ['/drums'], 'selected too');
  const top = s.drawn.levels[0];
  // Nothing grows and nothing moves: the drawn level is the stored one, box for box.
  assert.deepEqual(top.layout.nodes.map((n) => [n.id, n.x, n.y, n.w, n.h]), before);
  assert.deepEqual(boxes(s, ''), before);
  // The camera frames the package (the owner's verdict on 9e81dc8: a microscope): centred, filling the stage less its
  // margins (or its die at DIE_MAX_K, a small inside); its die scaled into it, uniformly, within it.
  const die = s.drawn.levels[1];
  assert.equal(die.path, '/drums');
  const r = rectOf(s, '', 'drums');
  assert.ok(Math.abs(r.x + r.w / 2 - viewport.w / 2) < 1e-6 && Math.abs(r.y + r.h / 2 - viewport.h / 2) < 1e-6, `centred: ${JSON.stringify(r)}`);
  assert.ok(Math.abs(r.w - (viewport.w - 48)) < 1e-6 || Math.abs(r.h - (viewport.h - 48)) < 1e-6 || Math.abs(die.transform.k - DIE_MAX_K) < 1e-9, `fills the stage: ${JSON.stringify(r)}`);
  const b = die.layout.bounds;
  const at = toScreen(die.transform, b);
  assert.ok(at.x >= r.x - 1e-6 && at.y >= r.y - 1e-6 && at.x + b.w * die.transform.k <= r.x + r.w + 1e-6 && at.y + b.h * die.transform.k <= r.y + r.h + 1e-6, 'its die within its package');
  assert.ok(dieOpacity(die.transform.k) > 0, 'its die drawn');
  // Recursion: a click on kick in the die zooms into kick the same way, drums staying open.
  click(s, middleOf(s, '/drums', 'kick'));
  assert.deepEqual([...s.drawn.open].sort(), ['/drums', '/drums/kick']);
  assert.ok(s.drawn.levels[2].transform.k >= FULL_K - 1e-9);
  assert.deepEqual(boxes(s, '/drums'), s.layoutOf('drums').nodes.map((n) => [n.id, n.x, n.y, n.w, n.h]));
  // A primitive in it is only selected.
  click(s, middleOf(s, '/drums/kick', 'osc'));
  assert.deepEqual([...s.drawn.open].sort(), ['/drums', '/drums/kick']);
  assert.deepEqual(s.selected.map((p) => p.address), ['/drums/kick/osc']);
});

test('a library composite only selects; a Shift-click and a drag do not zoom', () => {
  const s = schematic();
  click(s, middleOf(s, '', 'lib'));
  assert.deepEqual([...s.pinned], []);
  assert.deepEqual(s.selected.map((p) => p.address), ['/lib']);
  const at = middleOf(s, '', 'drums');
  click(s, at, { x: at.x + 40, y: at.y + 10 });
  assert.deepEqual([...s.pinned], [], 'dragged: not zoomed into');
  assert.deepEqual([...s.drawn.open], []);
  time += 1000;
  s.pointerDown({ button: 0, pointerId: 1, clientX: at.x, clientY: at.y, shiftKey: true, metaKey: false, ctrlKey: false, timeStamp: time });
  s.pointerUp({ button: 0, pointerId: 1, clientX: at.x, clientY: at.y, shiftKey: true, metaKey: false, ctrlKey: false, timeStamp: time });
  assert.deepEqual([...s.pinned], [], 'Shift-click: added to the selection only');
});

test('Escape ends a trace, then zooms out of the innermost chip, one level at a time, then lets the selection go', () => {
  const s = schematic();
  const fitted = topK(s);
  click(s, middleOf(s, '', 'drums'));
  const drumsK = topK(s);
  click(s, middleOf(s, '/drums', 'kick'));
  s.tracing = { result: null };
  key(s, 'Escape');
  assert.equal(s.ended, 1, 'the trace ended first');
  assert.deepEqual([...s.drawn.open].sort(), ['/drums', '/drums/kick']);
  key(s, 'Escape');
  assert.deepEqual([...s.drawn.open], ['/drums'], 'out of kick, the innermost');
  assert.ok(Math.abs(topK(s) - drumsK) < 1e-9 * drumsK, `back to drums framed: ${topK(s)} vs ${drumsK}`);
  assert.deepEqual(s.selected.map((p) => p.address), ['/drums/kick'], 'the selection kept');
  key(s, 'Escape');
  assert.deepEqual([...s.drawn.open], []);
  assert.ok(Math.abs(topK(s) - fitted) < 1e-9 * fitted, 'back to the whole board');
  key(s, 'Escape');
  assert.deepEqual(s.selected, [], 'then the selection');
});

test('zooming out past a die’s threshold leaves it; zooming in opens it by itself (2026-10-31), a pan opens and closes nothing', () => {
  const s = schematic();
  const at = middleOf(s, '', 'drums');
  for (let i = 0; i < 40 && !s.drawn.open.has('/drums/kick'); i++) s.zoomBy(1.2, at);
  assert.ok(s.drawn.open.has('/drums'), 'zoomed in on drums: its die opened without a click');
  assert.ok(s.drawn.open.has('/drums/kick'), 'and kick’s inside it, recursively');
  assert.deepEqual(s.selected, [], 'nothing selected');
  s.fitTop();
  click(s, middleOf(s, '', 'drums'));
  click(s, middleOf(s, '/drums', 'kick'));
  assert.deepEqual([...s.drawn.open].sort(), ['/drums', '/drums/kick']);
  // Out step by step: a die goes when its text falls under its threshold or it is small on the stage.
  const seen = [];
  for (let i = 0; i < 60 && s.drawn.open.size; i++) {
    s.zoomBy(1 / 1.2, { x: 500, y: 300 });
    seen.push([...s.drawn.open].sort().join(' '));
  }
  // Innermost first (a die opens at its natural size, so kick's, larger than drums', may take drums with it).
  const steps = [...new Set(seen)];
  assert.equal(steps.at(-1), '', 'every die left');
  assert.ok(!steps.includes('/drums/kick'), 'never the outer one first');
  // A pan changes nothing open.
  click(s, middleOf(s, '', 'drums'));
  s.setView({ ...s.view, x: s.view.x + 300 });
  assert.deepEqual([...s.drawn.open], ['/drums']);
});

test('a double-click zooms in as a click does, and leaves the selection as it was (2026-10-31); its second click opens nothing inside', () => {
  const s = schematic();
  const wire = s.drawn.levels[0].layout.wires.find((w) => w.written.from === '/drums.out');
  s.selected = [{ kind: 'wire', path: '', level: '', wire, from: '/drums.out', to: '/out.l' }];
  const at = middleOf(s, '', 'drums');
  click(s, at);
  // The second click of the double-click lands in the die now there.
  click(s, at, null, 100);
  assert.deepEqual([...s.pinned], ['/drums'], 'nothing more opened');
  s.doubleClick({ clientX: at.x, clientY: at.y, timeStamp: time });
  assert.deepEqual([...s.drawn.open], ['/drums']);
  assert.ok(s.drawn.levels[1].transform.k >= FULL_K - 1e-9, 'readable');
  assert.deepEqual(s.selected.map((p) => [p.kind, p.from, p.to]), [['wire', '/drums.out', '/out.l']], 'the wire still selected');
  // A single click still selects.
  click(s, middleOf(s, '/drums', 'kick'));
  assert.deepEqual(s.selected.map((p) => p.address), ['/drums/kick']);
});

// 2026-10-31 (the owner: "double-clicking a chip also changes the selection, so you cannot follow a signal all the way
// into the chip; the microscope view should appear by itself while zooming in, its inside fading in, the inverse of the
// fade-out"): semantic zoom, through the element's real wheel and pinch path (`zoomBy`) and render.

/** /drums' die's shown opacity as drawn (0 when it is not open). */
const drumsShown = (s) => s.drawn.levels.find((l) => l.path === '/drums')?.opacity ?? 0;

test('semantic zoom: zooming never changes the selection, in or out, through every die it opens and leaves', () => {
  const s = schematic();
  const wire = s.drawn.levels[0].layout.wires.find((w) => w.written.from === '/drums.out');
  const selected = [{ kind: 'wire', path: '', level: '', wire, from: '/drums.out', to: '/out.l' }];
  s.selected = selected;
  const at = middleOf(s, '', 'drums');
  const opened = new Set();
  for (let i = 0; i < 40; i++) {
    s.zoomBy(1.15, at);
    for (const p of s.drawn.open) opened.add(p);
    assert.equal(s.selected, selected, `zoom step ${i}: the selection untouched`);
  }
  assert.ok(opened.has('/drums') && opened.has('/drums/kick'), 'the zoom opened drums and kick');
  // The selected wire followed into the dies drawn (raised there): drums' kick.out → out, then kick's dc.out → out.
  const followed = wireNet(s.drawn.levels, selected[0]).map(({ path, wire: w }) => `${path}: ${w.from.port} → ${w.to.port}`);
  assert.deepEqual(followed, ['/drums: out → out', '/drums/kick: out → out']);
  for (let i = 0; i < 60 && s.drawn.open.size; i++) {
    s.zoomBy(1 / 1.15, at);
    assert.equal(s.selected, selected, `zoom out step ${i}: the selection untouched`);
  }
  assert.equal(s.drawn.open.size, 0, 'every die left');
  assert.equal(s.selected, selected);
});

/** KICK's stub board is small: fitted, drums' die would already be whole. Zoomed out to a quarter, it is under its band. */
function below(s) {
  s.setView(zoomAt(s.view, 1 / 4, middleOf(s, '', 'drums')));
  assert.equal(s.drawn.open.size, 0);
}

test('semantic zoom: the die fades in monotonically across the band, from nothing, and out along the same curve (symmetric)', () => {
  const s = schematic();
  below(s);
  const at = middleOf(s, '', 'drums');
  const steps = 30;
  const into = [drumsShown(s)];
  for (let i = 0; i < steps; i++) {
    s.zoomBy(1.1, at);
    into.push(drumsShown(s));
  }
  const out = [drumsShown(s)];
  for (let i = 0; i < steps; i++) {
    s.zoomBy(1 / 1.1, at);
    out.push(drumsShown(s));
  }
  out.reverse();
  for (let i = 1; i < into.length; i++) {
    assert.ok(into[i] >= into[i - 1] - 1e-9, `monotonic in: ${into[i - 1]} → ${into[i]}`);
    assert.ok(into[i] - into[i - 1] <= 0.3, `no jump in: ${into[i - 1]} → ${into[i]}`);
  }
  assert.equal(into[0], 0, 'closed when fitted');
  assert.equal(into.at(-1), 1, 'whole at the end');
  assert.ok(into.some((o) => o > 0 && o < 1), 'through the band, not a step');
  // The way out is the way in, step for step.
  for (let i = 0; i < into.length; i++) assert.ok(Math.abs(into[i] - out[i]) < 1e-6, `symmetric at step ${i}: in ${into[i]}, out ${out[i]}`);
});

test('semantic zoom: while its die is mostly transparent a click hits the chip (and travels into it); shown, the die’s parts are hit', () => {
  const s = schematic();
  below(s);
  const at = middleOf(s, '', 'drums');
  for (let i = 0; i < 80 && !(drumsShown(s) > 0); i++) s.zoomBy(1.03, at);
  const faint = drumsShown(s);
  assert.ok(faint > 0 && faint < HIT_OPACITY, `faint: ${faint}`);
  const centre = middleOf(s, '', 'drums');
  assert.equal(hitTest(s.drawn, centre).address, '/drums', 'the chip is hit');
  click(s, centre);
  assert.ok(drumsShown(s) === 1, 'the click travelled in');
  const kick = middleOf(s, '/drums', 'kick');
  const hit = hitTest(s.drawn, kick);
  assert.deepEqual([hit.path, hit.address], ['/drums', '/drums/kick'], 'the die’s chip is hit');
});

test('semantic zoom: no layout per frame; each level is laid out once, however long the zoom goes on', () => {
  const counter = { laid: 0 };
  const s = schematic(counter);
  const at = middleOf(s, '', 'drums');
  const round = () => {
    for (let i = 0; i < 40; i++) s.zoomBy(1.15, at);
    for (let i = 0; i < 40; i++) s.zoomBy(1 / 1.15, at);
  };
  round();
  const once = counter.laid;
  assert.ok(once <= 4, `the top level, drums, kick and the library composite at most: ${once}`);
  round();
  round();
  assert.equal(counter.laid, once, 'none laid out again');
});

test('Fit and a breadcrumb zoom out to their level and leave what is below them', () => {
  const s = schematic();
  click(s, middleOf(s, '', 'drums'));
  const drumsK = topK(s);
  click(s, middleOf(s, '/drums', 'kick'));
  s.goTo('/drums');
  assert.deepEqual([...s.drawn.open], ['/drums'], 'the breadcrumb’s level stays');
  assert.ok(Math.abs(topK(s) - drumsK) < 1e-9 * drumsK, 'framed as its click framed it');
  s.goTo('');
  assert.deepEqual([...s.pinned], []);
  assert.deepEqual([...s.drawn.open], []);
  click(s, middleOf(s, '', 'drums'));
  s.fitTop();
  assert.deepEqual([...s.pinned], []);
  assert.deepEqual([...s.drawn.open], []);
});

// ADR 0034 (review of c89253a, 10): the selection sidebar's parameter
// controls, a selector of a value set's names or a slider, each carry the
// parameter's name for a screen reader.
test('the sidebar’s parameter selector and slider are named for a screen reader', async () => {
  const { readFileSync } = await import('node:fs');
  const source = readFileSync(new URL('./schematic.js', import.meta.url), 'utf8');
  const made = source.indexOf("const input = isDiscrete(row) ? html('select', {})");
  assert.ok(made > 0, 'the sidebar makes its selector or slider');
  const named = source.indexOf("input.setAttribute('aria-label', row.name);", made);
  assert.ok(named > made && named - made < 400, 'and names it at once');
});

/** An SVG element, as much of one as `buildLabels` writes. */
class FakeSvg {
  constructor(name) {
    this.name = name;
    this.attributes = {};
    this.children = [];
    this.text = '';
  }
  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }
  getAttribute(name) {
    return this.attributes[name] ?? null;
  }
  removeAttribute(name) {
    delete this.attributes[name];
  }
  append(...children) {
    this.children.push(...children);
  }
  replaceChildren(...children) {
    this.children = children;
  }
  set textContent(value) {
    this.text = value;
  }
  /** Its `text` elements' lines, itself and below. */
  texts() {
    return [...(this.name === 'text' ? [this.text] : []), ...this.children.flatMap((c) => c.texts())];
  }
}

test('a primitive’s box is drawn with its pins’ labels (a pin’s tooltip once shadowed the drawing helper, 2026-10-28)', () => {
  const port = (name, direction) => ({ name, direction, shape: 'signal', kind: 'audio', rate: 1 });
  const drawnLayout = layout({ instances: [{ name: 'osc', module: 'std.osc', ports: [port('hz', 'in'), port('out', 'out')], parameters: [] }], connections: [] });
  const had = globalThis.document;
  globalThis.document = { createElementNS: (_, name) => Object.assign(new FakeSvg(name), { dataset: {} }) };
  try {
    const s = Object.create(SignalSchematic.prototype);
    Object.assign(s, { drawn: { open: new Set() }, tracing: null, graph: {}, mutedSet: new Set(), selected: [], barsOf: new WeakMap() });
    const transform = { k: 1, x: 0, y: 0 };
    const level = { path: '', level: '', layout: drawnLayout, transform, collapsed: false };
    const node = drawnLayout.nodes.find((n) => n.name === 'osc');
    const drawn = s.nodeElement(level, node, bake(transform, viewport), () => true);
    const group = drawn.el ?? drawn;
    const texts = group.texts();
    for (const name of ['osc', 'hz', 'out']) assert.ok(texts.includes(name), `${name} in ${JSON.stringify(texts)}`);
  } finally {
    globalThis.document = had;
  }
});

test('the level’s labels are drawn in a group per box and size, kept by its id in `labelCache`, which the drag moves with the box (the owner’s 2026-10-27)', () => {
  const port = (name, direction) => ({ name, direction, shape: 'signal', kind: 'audio', rate: 1 });
  const instance = (name, ins, outs, composite) => ({ name, module: composite ? `${name}_type` : `std.${name}`, composite, ports: [...ins.map((p) => port(p, 'in')), ...outs.map((p) => port(p, 'out'))], parameters: [] });
  // A composite with pins, a freshly added empty composite, and a primitive (whose pins' labels are in its own box's group).
  const drawnLayout = layout({ instances: [instance('voice', ['hz', 'gate'], ['out'], true), instance('e', [], [], true), instance('osc', ['in'], ['out'], false)], connections: [] });
  const had = globalThis.document;
  globalThis.document = { createElementNS: (_, name) => new FakeSvg(name) };
  try {
    const s = Object.create(SignalSchematic.prototype);
    Object.assign(s, { drawn: { open: new Set() }, tracing: null, graph: {} });
    const transform = { k: 1, x: 0, y: 0 };
    const level = { path: '', level: '', layout: drawnLayout, transform, collapsed: false };
    const label = (detail) => {
      const drawn = { baked: bake(transform, viewport), labels: new FakeSvg('g') };
      drawn.labelBoxes = s.labelBoxes(level, { detail, notes: false, hovered: null });
      s.buildLabels(level, drawn, { detail, counts: null, viewport: { x: 0, y: 0, w: 1e6, h: 1e6 } });
      return drawn;
    };
    for (const detail of ['pins', 'summary']) {
      const drawn = label(detail);
      const keys = [...drawn.labelCache.keys()].sort();
      // At every pin's detail the empty composite has no label (no pins); its summary is its one label.
      assert.deepEqual(keys, detail === 'pins' ? ['voice'] : ['e', 'voice'], `${detail}: one entry per labelled composite, none for the primitive`);
      for (const [key, { ids, elements }] of drawn.labelCache) {
        assert.deepEqual(ids, [key]);
        for (const g of elements) {
          assert.equal(g.getAttribute('data-box'), key);
          assert.equal(drawn.labels.children.filter((sized) => sized.children.includes(g)).length, 1, `${detail}: ${key}’s group is in one size’s group`);
        }
      }
      const texts = (key) => drawn.labelCache.get(key).elements.flatMap((g) => g.texts());
      if (detail === 'pins') assert.deepEqual(texts('voice').sort(), ['gate', 'hz', 'out']);
      else assert.ok(texts('voice').join(' ').startsWith('voice'), texts('voice').join(' '));
      if (detail === 'summary') assert.equal(texts('e').join(' '), 'e: 0 in · 0 out', 'the empty composite’s summary');
      // No label of a box is left outside its box's group.
      const all = drawn.labels.children.flatMap((g) => g.texts());
      const owned = [...drawn.labelCache.values()].flatMap(({ elements }) => elements.flatMap((g) => g.texts()));
      assert.deepEqual(all.sort(), owned.sort(), detail);
      if (detail === 'pins') continue;
      // Dragged, the empty composite's summary goes with its box.
      const nodeCache = new Map(drawnLayout.nodes.map((n) => [n.id, { el: new FakeSvg('g') }]));
      const groups = new Map([['', { ...drawn, nodeCache, wireCache: new Map() }]]);
      const node = drawnLayout.nodes.find((n) => n.name === 'e');
      new DragFollow('', 'e').apply({ layout: drawnLayout, levels: [level], groups, to: { x: node.x + 20, y: node.y + 5 } });
      const moved = nodeCache.get('e').el.getAttribute('transform');
      assert.ok(moved);
      for (const g of drawn.labelCache.get('e').elements) assert.equal(g.getAttribute('transform'), moved, `${detail}: e’s labels follow`);
      for (const g of drawn.labelCache.get('voice').elements) assert.equal(g.getAttribute('transform'), null, `${detail}: voice’s stay`);
    }
  } finally {
    globalThis.document = had;
  }
});

test('ADR 0047 phase 4: a graph with the same structure (a value changed) keeps the very layout of every level; a moved instance reroutes only its own wires', () => {
  const s = Object.create(SignalSchematic.prototype);
  const graph = JSON.parse(JSON.stringify({ project: 'kick', projectId: 'k', top: KICK.top, composites: KICK.composites, layout: { positions: {} } }));
  Object.assign(s, { graph, layouts: new Map(), moved: new Map(), panelControls: [], board: null, faces: null });
  const top = s.layoutOf('');
  const inner = Object.keys(KICK.composites)[0];
  const die = s.layoutOf(inner);
  assert.ok(top.board && die.board);
  // A new graph (as `setGraph` clears the cache) whose only change is a parameter's value: the same objects, no layout.
  const valued = JSON.parse(JSON.stringify(graph));
  for (const i of valued.top.instances) for (const p of i.parameters ?? []) p.default = 0.123;
  s.graph = valued;
  s.layouts.clear();
  assert.equal(s.layoutOf(''), top, 'the top level: the very object');
  assert.equal(s.layoutsHow, 'kept');
  assert.equal(s.layoutOf(inner), die, 'its die too');
  // An instance moved (its stored position written): only its wires are routed again.
  const chip = top.nodes.find((n) => n.kind === 'instance');
  const movedGraph = JSON.parse(JSON.stringify(valued));
  movedGraph.layout = { positions: { '': { [chip.name]: { x: chip.x + 16, y: chip.y + 32 } } } };
  s.graph = movedGraph;
  s.layouts.clear();
  const after = s.layoutOf('');
  assert.equal(s.layoutsHow, 'rerouted');
  for (const w of top.wires) {
    if (w.from.node === chip.id || w.to.node === chip.id || after.rerouted.includes(w.id)) continue;
    assert.deepEqual(after.wires.find((x) => x.id === w.id).corners, w.corners, `${w.id} as it was`);
  }
  for (const n of after.nodes) if (n.id !== chip.id) assert.deepEqual([n.x, n.y], [top.nodes.find((m) => m.id === n.id).x, top.nodes.find((m) => m.id === n.id).y]);
});

test('ADR 0047 phase 4: the studio on its plate, a new graph with the same structure keeps its board and its synth’s die (laid in its package’s order)', async () => {
  const { readFileSync } = await import('node:fs');
  const { approximatePlate } = await import('../lib/fixtures/board/plate-approx.js');
  const read = (path) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');
  const studio = JSON.parse(read('ui/lib/fixtures/studio-graph.json'));
  const fx = JSON.parse(read('ui/lib/fixtures/board/studio-plate.json'));
  const approx = approximatePlate(read('projects/studio/ui/main.html'));
  const s = Object.create(SignalSchematic.prototype);
  const board = { plate: { key: 'measured' }, option: () => ({ vias: new Map(fx.vias), obstacles: fx.obstacles, plate: fx.size, cards: fx.cards }) };
  Object.assign(s, { graph: { project: 'studio', projectId: 's', ...studio, layout: { positions: {} } }, layouts: new Map(), moved: new Map(), panelControls: approx.controls, board, faces: null });
  const top = s.layoutOf('');
  const die = s.layoutOf('moog_voice');
  assert.ok(top.nodes.find((n) => n.name === 'synth').topOrder, 'the synth has top pins (its vias)');
  s.graph = { ...s.graph, top: JSON.parse(JSON.stringify(studio.top)) };
  s.layouts.clear();
  s.dieOrders.clear();
  assert.equal(s.layoutOf(''), top);
  assert.equal(s.layoutOf('moog_voice'), die, 'the die as it was, in its package’s order');
});
