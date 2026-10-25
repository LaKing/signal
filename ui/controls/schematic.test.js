// <signal-schematic>'s opening and closing (2026-10-22, the owner: "when I
// zoom in, a composite opens automatically. It would be better if it
// opened on a click, and there were an × in the top-right corner to close
// it"), driven through its real pointer, key, zoom and render code with the
// least of a DOM it needs: the drawing itself (`drawLevel` and the marks)
// is left out, the scene is the real one.

import test from 'node:test';
import assert from 'node:assert/strict';
import { layout } from '../lib/schematic-layout.js';
import { CLOSE_MIN_PX, READ_K, closeBox } from '../lib/schematic-open.js';
import { hitTest } from '../lib/schematic-hit.js';
import { fit, toScreen } from '../lib/schematic-view.js';
import { KICK } from '../lib/schematic-scene.test.js';

globalThis.HTMLElement ??= class {};
globalThis.customElements ??= { get: () => undefined, define: () => {} };

const { SignalSchematic } = await import('./schematic.js');

const viewport = { w: 1000, h: 600 };

/** KICK's levels, and a library composite `/lib` (its inside not in the graph) beside them. */
function kickLayouts() {
  const top = { ...KICK.top, instances: [...KICK.top.instances, { name: 'lib', module: 'lib.voice', composite: true, ports: [], parameters: [] }] };
  const layouts = new Map();
  return (level) => {
    if (!layouts.has(level)) layouts.set(level, level === '' ? layout(top) : KICK.composites[level] ? layout(KICK.composites[level]) : null);
    return layouts.get(level);
  };
}

/** A schematic showing KICK fitted (as on loading), every redraw its real `render` without the drawing. */
function schematic() {
  const s = Object.create(SignalSchematic.prototype);
  const layoutOf = kickLayouts();
  Object.assign(s, {
    graph: { project: 'kick' },
    viewport,
    view: fit(layoutOf('').bounds, viewport),
    fitted: true,
    open: new Set(),
    pinned: new Set(),
    openShifts: new Map(),
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

/** The middle of instance `name`'s box at level `path` on screen. */
function middleOf(s, path, name) {
  const level = s.drawn.levels.find((l) => l.path === path);
  const node = level.stored.nodes.find((n) => n.name === name);
  return toScreen(level.transform, { x: node.x + node.w / 2, y: node.y + node.h / 2 });
}

/** The middle of the × of the open composite at `path` on screen. */
function closeOf(s, path) {
  const cut = path.lastIndexOf('/');
  const level = s.drawn.levels.find((l) => l.path === path.slice(0, cut));
  const r = closeBox(level.layout.nodes.find((n) => n.name === path.slice(cut + 1)), level.transform.k);
  return toScreen(level.transform, { x: r.x + r.w / 2, y: r.y + r.h / 2 });
}

/** The scale of the top level on screen. */
const topK = (s) => s.drawn.levels[0].transform.k;

test('a zoom never opens nor closes: in a thousandfold about a closed composite, out a thousandfold from an open one', () => {
  const s = schematic();
  const at = middleOf(s, '', 'drums');
  for (let i = 0; i < 40; i++) s.zoomBy(1.2, at);
  assert.deepEqual([...s.drawn.open], [], 'zoomed in on drums: still closed');
  assert.ok(topK(s) > 1000 * fit(s.layoutOf('').bounds, viewport).k);
  s.fitTop();
  click(s, middleOf(s, '', 'drums'));
  assert.deepEqual([...s.drawn.open], ['/drums']);
  for (let i = 0; i < 40; i++) s.zoomBy(1 / 1.2, { x: 500, y: 300 });
  assert.deepEqual([...s.drawn.open], ['/drums'], 'zoomed far out: still open');
  for (let i = 0; i < 80; i++) s.zoomBy(1.2, { x: 500, y: 300 });
  assert.deepEqual([...s.drawn.open], ['/drums'], 'and far in again');
});

test('a click on a closed composite’s box opens it where it is and selects it; the view is not zoomed', () => {
  const s = schematic();
  const k = topK(s);
  const at = middleOf(s, '', 'drums');
  const box = s.drawn.levels[0].stored.nodes.find((n) => n.name === 'drums');
  const fx = (at.x - toScreen(s.drawn.levels[0].transform, box).x) / (box.w * k);
  click(s, at);
  assert.deepEqual([...s.pinned], ['/drums']);
  assert.deepEqual([...s.drawn.open], ['/drums']);
  assert.deepEqual(s.selected.map((p) => p.address), ['/drums']);
  assert.equal(topK(s), k, 'the same scale');
  // The box's point under the pointer is the same point of its frame (across; down, its header may be kept on the stage).
  const top = s.drawn.levels[0];
  const frame = top.layout.nodes.find((n) => n.name === 'drums');
  const p = toScreen(top.transform, { x: frame.x + fx * frame.w, y: frame.y });
  assert.ok(Math.abs(p.x - at.x) < 1e-6, `${p.x} vs ${at.x}`);
  // Inside it, a click on kick opens kick too; on a primitive nothing opens.
  click(s, middleOf(s, '/drums', 'kick'));
  assert.deepEqual([...s.drawn.open].sort(), ['/drums', '/drums/kick']);
  click(s, middleOf(s, '/drums/kick', 'osc'));
  assert.deepEqual([...s.drawn.open].sort(), ['/drums', '/drums/kick']);
  assert.deepEqual(s.selected.map((p) => p.address), ['/drums/kick/osc']);
});

test('a library composite only selects; a Shift-click and a drag do not open', () => {
  const s = schematic();
  click(s, middleOf(s, '', 'lib'));
  assert.deepEqual([...s.pinned], []);
  assert.deepEqual(s.selected.map((p) => p.address), ['/lib']);
  const at = middleOf(s, '', 'drums');
  click(s, at, { x: at.x + 40, y: at.y + 10 });
  assert.deepEqual([...s.pinned], [], 'dragged: not opened');
  assert.deepEqual([...s.drawn.open], []);
  time += 1000;
  s.pointerDown({ button: 0, pointerId: 1, clientX: at.x, clientY: at.y, shiftKey: true, metaKey: false, ctrlKey: false, timeStamp: time });
  s.pointerUp({ button: 0, pointerId: 1, clientX: at.x, clientY: at.y, shiftKey: true, metaKey: false, ctrlKey: false, timeStamp: time });
  assert.deepEqual([...s.pinned], [], 'Shift-click: added to the selection only');
});

test('the × closes its composite and what is open inside it, selecting nothing, the view not zoomed', () => {
  const s = schematic();
  click(s, middleOf(s, '', 'drums'));
  click(s, middleOf(s, '/drums', 'kick'));
  assert.deepEqual([...s.drawn.open].sort(), ['/drums', '/drums/kick']);
  s.select([]);
  const k = topK(s);
  // kick's own × closes kick alone.
  click(s, closeOf(s, '/drums/kick'));
  assert.deepEqual([...s.drawn.open], ['/drums']);
  click(s, middleOf(s, '/drums', 'kick'));
  s.select([]);
  const at = closeOf(s, '/drums');
  click(s, at);
  assert.deepEqual([...s.pinned], [], 'drums and kick inside it');
  assert.deepEqual([...s.drawn.open], []);
  assert.deepEqual(s.selected, [], 'nothing selected');
  assert.equal(topK(s), k, 'the same scale');
  // A press on the × dragged away does not pan.
  click(s, middleOf(s, '', 'drums'));
  const view = s.view;
  const x = closeOf(s, '/drums');
  click(s, x, { x: x.x - 200, y: x.y + 200 });
  assert.deepEqual([...s.drawn.open], ['/drums'], 'let go elsewhere: still open');
  assert.equal(s.view, view, 'not panned');
  // Slid onto its frame's header beside it: not closed either.
  const side = closeBox(s.drawn.levels[0].layout.nodes.find((n) => n.name === 'drums'), topK(s)).w * topK(s);
  assert.ok(side >= CLOSE_MIN_PX - 1e-9, `the × ${side} px on screen at least`);
  click(s, x, { x: x.x - 2 * side, y: x.y });
  assert.equal(hitTest(s.drawn, { x: x.x - 2 * side, y: x.y }).kind, 'instance');
  assert.deepEqual([...s.drawn.open], ['/drums'], 'let go on the frame: still open');
});

test('Escape ends a trace, then closes the innermost open composite, one at a time, then lets the selection go', () => {
  const s = schematic();
  click(s, middleOf(s, '', 'drums'));
  click(s, middleOf(s, '/drums', 'kick'));
  s.tracing = { result: null };
  key(s, 'Escape');
  assert.equal(s.ended, 1, 'the trace ended first');
  assert.deepEqual([...s.drawn.open].sort(), ['/drums', '/drums/kick']);
  const k = topK(s);
  key(s, 'Escape');
  assert.deepEqual([...s.drawn.open], ['/drums'], 'kick, the innermost');
  assert.deepEqual(s.selected.map((p) => p.address), ['/drums/kick'], 'the selection kept');
  key(s, 'Escape');
  assert.deepEqual([...s.drawn.open], []);
  assert.equal(topK(s), k, 'the view not zoomed');
  key(s, 'Escape');
  assert.deepEqual(s.selected, [], 'then the selection');
});

test('closed by its ×, a composite’s box is back where it was before it opened, and a click there opens it again', () => {
  const s = schematic();
  const at = middleOf(s, '', 'drums');
  click(s, at);
  assert.deepEqual([...s.drawn.open], ['/drums']);
  click(s, closeOf(s, '/drums'));
  assert.deepEqual([...s.drawn.open], []);
  const back = middleOf(s, '', 'drums');
  assert.ok(Math.hypot(back.x - at.x, back.y - at.y) < 1e-6, `${back.x},${back.y} vs ${at.x},${at.y}`);
  click(s, at);
  assert.deepEqual([...s.drawn.open], ['/drums'], 'opened again');
  // Zoomed in after opening, closing still puts the box under the frame's region it grew from, at the scale now.
  for (let i = 0; i < 3; i++) s.zoomBy(1.2, closeOf(s, '/drums'));
  const k = topK(s);
  click(s, closeOf(s, '/drums'));
  assert.deepEqual([...s.drawn.open], []);
  assert.equal(topK(s), k, 'not zoomed');
});

test('a double-click opens and enters, readable; its second click opens nothing inside', () => {
  const s = schematic();
  const at = middleOf(s, '', 'drums');
  click(s, at);
  // The second click of the double-click lands in the frame now there.
  click(s, at, null, 100);
  assert.deepEqual([...s.pinned], ['/drums'], 'nothing more opened');
  assert.deepEqual(s.selected.map((p) => p.address), ['/drums'], 'nor selected');
  s.doubleClick({ clientX: at.x, clientY: at.y, timeStamp: time });
  assert.deepEqual([...s.drawn.open], ['/drums']);
  assert.ok(Math.abs(s.drawn.levels[1].transform.k - READ_K) < 1e-9 || s.drawn.levels[1].transform.k > READ_K, 'Enter’s view: readable');
});

test('Fit and a breadcrumb close what is below them', () => {
  const s = schematic();
  click(s, middleOf(s, '', 'drums'));
  click(s, middleOf(s, '/drums', 'kick'));
  s.goTo('/drums');
  s.render();
  assert.deepEqual([...s.drawn.open], ['/drums']);
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
