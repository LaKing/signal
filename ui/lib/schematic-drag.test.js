import test from 'node:test';
import assert from 'node:assert/strict';
import { DragFollow, innerOffset } from './schematic-drag.js';
import { bake, bakedMatrix, bakedPath } from './schematic-draw.js';
import { layout, routed, movedWires } from './schematic-layout.js';
import { focus, matrix } from './schematic-view.js';
import { OPEN_PX, nodeRect, rebase, reroot, scene } from './schematic-scene.js';
import { KICK } from './schematic-scene.test.js';

/** An element: its attributes, and how many writes it took. */
class Element {
  constructor(attributes = {}) {
    this.attributes = { ...attributes };
    this.writes = 0;
  }
  getAttribute(name) {
    return this.attributes[name] ?? null;
  }
  setAttribute(name, value) {
    this.attributes[name] = String(value);
    this.writes++;
  }
  removeAttribute(name) {
    delete this.attributes[name];
    this.writes++;
  }
}

const port = (name, direction) => ({ name, direction, shape: 'signal', kind: 'audio', rate: 1 });
const box = (name, ins = ['in'], outs = ['out']) => ({ name, module: `std.${name}`, composite: false, ports: [...ins.map((p) => port(p, 'in')), ...outs.map((p) => port(p, 'out'))], parameters: [] });
const wire = (from, to) => ({ from, to, scale: null, converters: [] });
const LEVEL = {
  instances: [box('a', []), box('b', ['in', 'back']), box('c'), box('d', ['in'], []), box('e', [])],
  connections: [wire('/a.out', '/b.in'), wire('/b.out', '/c.in'), wire('/c.out', '/d.in'), wire('/c.out', '/b.back')],
};
const viewport = { w: 1000, h: 600 };

/** A level drawn as <signal-schematic> keeps it: its group, bake, matrix and elements by key. */
function drawnGroup(drawn, transform) {
  const baked = bake(transform, viewport);
  const placed = bakedMatrix(baked, transform);
  return {
    g: new Element({ transform: matrix(placed) }),
    baked,
    placed,
    wireCache: new Map(drawn.wires.map((w) => [w.id, { el: new Element({ d: bakedPath(baked, w.points, w.curve) }) }])),
    nodeCache: new Map(drawn.nodes.map((n) => [n.id, { el: new Element() }])),
  };
}

function setup() {
  const drawn = layout(LEVEL);
  const transform = { k: 1.5, x: 40, y: 30 };
  const group = drawnGroup(drawn, transform);
  return { drawn, transform, group, groups: new Map([['', group]]), levels: [{ path: '', level: '', transform }] };
}
const d = (group, id) => group.wireCache.get(id).el.getAttribute('d');
const label = (drawn, text) => drawn.wires.find((w) => w.label === text).id;

test('a drag writes the box’s offset and the wires that change, each once, and a cancel puts them back', () => {
  const { drawn, group, groups, levels } = setup();
  const before = drawn.wires.map((w) => d(group, w.id));
  const c = drawn.nodes.find((n) => n.id === 'c');
  const follow = new DragFollow('', 'c');
  const to = { x: c.x + 30, y: c.y - 10 };
  const writes = follow.apply({ layout: drawn, levels, groups, to });
  const moved = movedWires(drawn, 'c', to);
  assert.equal(writes, 1 + moved.length, 'the box, then each wire that changed');
  assert.equal(group.nodeCache.get('c').el.getAttribute('transform'), `translate(${30 * group.baked.m} ${-10 * group.baked.m})`);
  for (const route of moved) {
    const { points, curve } = routed(route);
    assert.equal(d(group, route.wire.id), bakedPath(group.baked, points, curve), route.wire.label);
  }
  const still = drawn.wires.filter((w) => !moved.some((m) => m.wire === w));
  assert.ok(still.length > 0);
  for (const w of still) assert.equal(d(group, w.id), before[w.id], `${w.label} untouched`);
  // A redraw with nothing moved (a stream frame) writes nothing.
  assert.equal(follow.apply({ layout: drawn, levels, groups, to }), 0);
  // Cancelled: every path as it was, the box without an offset.
  follow.restore();
  assert.deepEqual(drawn.wires.map((w) => d(group, w.id)), before);
  assert.equal(group.nodeCache.get('c').el.getAttribute('transform'), null);
});

test('a wire that stops changing goes back to its own path', () => {
  const { drawn, group, groups, levels } = setup();
  const e = drawn.nodes.find((n) => n.id === 'e');
  const loop = label(drawn, '/c.out → /b.back');
  const own = d(group, loop);
  const follow = new DragFollow('', 'e');
  // e, attached to nothing, dragged below every box: the loop's lane goes down with it…
  follow.apply({ layout: drawn, levels, groups, to: { x: e.x, y: e.y + 900 } });
  assert.notEqual(d(group, loop), own);
  // …and back up: the loop is its own again, written once.
  const writes = group.wireCache.get(loop).el.writes;
  follow.apply({ layout: drawn, levels, groups, to: { x: e.x + 3, y: e.y } });
  assert.equal(d(group, loop), own);
  assert.equal(group.wireCache.get(loop).el.writes, writes + 1);
});

test('an open composite’s inside moves with its box, in every instance drawn, and stays inside SVG’s bounds', () => {
  const { drawn, transform, group } = setup();
  // c's type drawn twice (as if two open instances of a composite showed this level), c open in both.
  const other = drawnGroup(drawn, { k: 0.5, x: 600, y: 10 });
  const inside = (k) => ({ g: new Element({ transform: matrix({ k, x: 5, y: 7 }) }), placed: { k, x: 5, y: 7 } });
  const groups = new Map([['/p', group], ['/p/c', inside(0.2)], ['/p/c/z', inside(0.05)], ['/q', other], ['/q/c', inside(0.1)], ['/q/cc', inside(0.3)]]);
  const levels = [{ path: '/p', level: 'comp', transform }, { path: '/q', level: 'comp', transform: { k: 0.5, x: 600, y: 10 } }];
  const c = drawn.nodes.find((n) => n.id === 'c');
  const follow = new DragFollow('comp', 'c');
  follow.apply({ layout: drawn, levels, groups, to: { x: c.x + 10, y: c.y + 4 } });
  const t = (path) => groups.get(path).g.getAttribute('transform');
  assert.equal(t('/p/c'), matrix(innerOffset({ k: 0.2, x: 5, y: 7 }, { x: 10, y: 4 }, transform.k)));
  assert.equal(t('/p/c/z'), matrix(innerOffset({ k: 0.05, x: 5, y: 7 }, { x: 10, y: 4 }, transform.k)), 'and what is open inside it');
  assert.equal(t('/q/c'), matrix(innerOffset({ k: 0.1, x: 5, y: 7 }, { x: 10, y: 4 }, 0.5)), 'the other instance too');
  assert.equal(t('/q/cc'), matrix({ k: 0.3, x: 5, y: 7 }), 'not a box of another name');
  assert.ok(other.nodeCache.get('c').el.getAttribute('transform'), 'the other instance’s box follows');
  // A redraw puts the insides at their matrices; the next step moves them again, and nothing else.
  for (const path of ['/p/c', '/p/c/z', '/q/c']) groups.get(path).g.setAttribute('transform', matrix(groups.get(path).placed));
  assert.equal(follow.apply({ layout: drawn, levels, groups, to: { x: c.x + 10, y: c.y + 4 } }), 3);
  // Out of SVG's bounds (a zoom since), an inside stays where it is drawn.
  const far = new Map([['/p', group], ['/p/c', inside(0.2)]]);
  const zoomed = new DragFollow('comp', 'c');
  zoomed.apply({ layout: drawn, levels: [{ path: '/p', level: 'comp', transform: { ...transform, k: 1e9 } }], groups: far, to: { x: c.x + 10, y: c.y } });
  assert.equal(far.get('/p/c').g.getAttribute('transform'), matrix({ k: 0.2, x: 5, y: 7 }));
  // Cancelled: every inside at its matrix again.
  follow.restore();
  for (const path of ['/p/c', '/p/c/z', '/q/c']) assert.equal(t(path), matrix(groups.get(path).placed));
});

test('released inside a composite the view is in, that inside stays where it is', () => {
  const layouts = new Map();
  const layoutOf = (level) => {
    if (!layouts.has(level)) layouts.set(level, layout(level === '' ? KICK.top : KICK.composites[level]));
    return layouts.get(level);
  };
  const node = layoutOf('').nodes.find((n) => n.name === 'drums');
  const k = (OPEN_PX + 1) / node.w;
  const opened = { k, x: viewport.w / 2 - (node.x + node.w / 2) * k, y: viewport.h / 2 - (node.y + node.h / 2) * k };
  const view = scene({ layoutOf, view: focus(opened, nodeRect(scene({ layoutOf, view: opened, viewport }), '/drums/kick'), viewport), viewport }).view;
  assert.equal(view.path, '/drums/kick');
  const kickAt = (drawn) => drawn.levels.find((l) => l.path === '/drums/kick').transform;
  const before = scene({ layoutOf, view, viewport });
  // osc dropped far below: kick's bounds, so its fit in its box, change.
  const osc = layoutOf('kick').nodes.find((n) => n.name === 'osc');
  const moved = layout(KICK.composites.kick, { osc: { x: osc.x, y: osc.y + 400 } });
  const after = (level) => (level === 'kick' ? moved : layoutOf(level));
  const kept = scene({ layoutOf: after, view: reroot(view, layoutOf, '/drums/kick'), viewport });
  const was = kickAt(before);
  const now = kickAt(kept);
  for (const key of ['k', 'x', 'y']) assert.ok(Math.abs(now[key] - was[key]) < 1e-9, `${key} ${now[key]} vs ${was[key]}`);
  // Kept from outside instead, the inside is fitted to its box again.
  const refit = kickAt(scene({ layoutOf: after, view: rebase(view, layoutOf, after), viewport }));
  assert.ok(Math.abs(refit.k - was.k) > 1e-6);
  assert.deepEqual(reroot(view, layoutOf, '/nope'), view);
});

// 2026-10-17: a group of boxes of one level dragged by one of them.
test('a group’s boxes move together by the dragged one’s offset, and come back on a cancel', () => {
  const drawn = layout(LEVEL);
  const transform = { k: 1, x: 0, y: 0 };
  const group = drawnGroup(drawn, transform);
  const groups = new Map([['', group]]);
  const levels = [{ path: '', level: '', transform }];
  const follow = new DragFollow('', ['b', 'c']);
  const b = drawn.nodes.find((n) => n.id === 'b');
  const to = { x: b.x + 10, y: b.y + 20 };
  follow.apply({ layout: drawn, levels, groups, to });
  const m = group.baked.m;
  for (const id of ['b', 'c']) assert.equal(group.nodeCache.get(id).el.getAttribute('transform'), `translate(${10 * m} ${20 * m})`, id);
  assert.equal(group.nodeCache.get('a').el.getAttribute('transform'), null, 'a box not in the group stays');
  const c = drawn.nodes.find((n) => n.id === 'c');
  const expected = movedWires(drawn, new Map([['b', to], ['c', { x: c.x + 10, y: c.y + 20 }]]));
  for (const route of expected) {
    const { points, curve } = routed(route);
    assert.equal(group.wireCache.get(route.wire.id).el.getAttribute('d'), bakedPath(group.baked, points, curve), route.wire.label);
  }
  assert.equal(follow.apply({ layout: drawn, levels, groups, to }), 0, 'nothing moved, nothing written');
  follow.restore();
  for (const id of ['b', 'c']) assert.equal(group.nodeCache.get(id).el.getAttribute('transform'), null);
  for (const w of drawn.wires) assert.equal(group.wireCache.get(w.id).el.getAttribute('d'), bakedPath(group.baked, w.points, w.curve));
});
