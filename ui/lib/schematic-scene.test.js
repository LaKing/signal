import test from 'node:test';
import assert from 'node:assert/strict';
import { layout } from './schematic-layout.js';
import { MIN_PX, boundaryLinks, breadcrumbs, inside, levelRect, nodeRect, planPort, rebase, scene } from './schematic-scene.js';
import { READ_K, grownNode, linkScale, pinOne } from './schematic-open.js';
import { belowThreshold } from './board-zoom.js';
import { bake, bakeHolds, bakedMatrix, bakedRect, SVG_MAX_SCALE, SVG_MAX_TRANSLATE, SVG_MIN_SCALE } from './schematic-draw.js';
import { hitTest, reselect } from './schematic-hit.js';
import { fit, focus, pan, screenRect, toScreen, zoomAt } from './schematic-view.js';

const p = (name, direction, shape = 'signal') => ({ name, direction, shape, kind: 'audio', rate: 1 });
const primitive = (name, module, ins, outs, parameters = []) => ({
  name, module, composite: false, ports: [...ins.map((n) => p(n, 'in')), ...outs.map((n) => p(n, 'out'))], parameters: parameters.map((n) => ({ name: n, rate: 1 })),
});

/** kick's shape, as `graph` gives it: drums holds kick, kick holds primitives. */
export const KICK = {
  top: {
    instances: [
      primitive('lfo', 'std.lfo', [], ['out']),
      { name: 'drums', module: 'drums', composite: true, ports: [p('trigger', 'in', 'events'), p('out', 'out')], parameters: [{ name: 'kick_decay', rate: 1 }] },
      primitive('out', 'std.audio_out', ['l', 'r'], [], ['gain']),
    ],
    connections: [
      { from: '/lfo.out', to: '/drums.kick_decay', scale: 120, converters: [] },
      { from: '/drums.out', to: '/out.l', scale: null, converters: [] },
    ],
  },
  composites: {
    drums: {
      instances: [{ name: 'kick', module: 'kick', composite: true, ports: [p('trigger', 'in', 'events'), p('out', 'out')], parameters: [{ name: 'decay', rate: 1 }] }],
      connections: [],
      inputs: [{ name: 'trigger', to: ['kick.trigger'], shape: 'events' }],
      outputs: [{ name: 'out', from: 'kick.out', shape: 'signal' }],
      parameters: [{ name: 'kick_decay', to: ['kick.decay'] }],
    },
    kick: {
      instances: [primitive('osc', 'std.sine_osc', ['pm'], ['out']), primitive('env', 'std.exp_env', ['trigger'], ['out'], ['decay_ms']), primitive('dc', 'std.dc_block', ['in'], ['out'])],
      connections: [{ from: 'osc.out', to: 'dc.in', scale: null, converters: [] }],
      inputs: [{ name: 'trigger', to: ['env.trigger'], shape: 'events' }],
      outputs: [{ name: 'out', from: 'dc.out', shape: 'signal' }],
      parameters: [{ name: 'decay', to: ['env.decay_ms'] }],
    },
  },
};

const layouts = new Map();
const layoutOf = (level) => {
  if (!layouts.has(level)) layouts.set(level, layout(level === '' ? KICK.top : KICK.composites[level]));
  return layouts.get(level);
};
const viewport = { w: 1000, h: 600 };
const drums = () => layoutOf('').nodes.find((n) => n.name === 'drums');

const DRUMS = new Set(['/drums']);
const KICK_OPEN = new Set(['/drums', '/drums/kick']);

/**
 * The view at `factor` times the scale at which drums' inside, open, is
 * drawn at READ_K (Enter's readability), drums centred.
 */
function drumsAt(factor) {
  const node = drums();
  const k = (READ_K / linkScale(node, layoutOf('drums').bounds)) * factor;
  return { k, x: viewport.w / 2 - (node.x + node.w / 2) * k, y: viewport.h / 2 - (node.y + node.h / 2) * k };
}

/** Opened drums, then the view focused on kick inside it, as a double-click does. */
function kickFocused() {
  const view = drumsAt(1.01);
  return focus(view, nodeRect(scene({ layoutOf, view, viewport, pinned: DRUMS }), '/drums/kick'), viewport);
}

// 2026-10-22: nothing opens unless the person opens it, so a plain fit
// (Fit) shows every composite closed.
test('fitted, only the top level is drawn: composites are closed boxes', () => {
  const drawn = scene({ layoutOf, view: fit(layoutOf('').bounds, viewport), viewport });
  assert.equal(drawn.levels.length, 1);
  assert.deepEqual(drawn.levels[0].nodes.map((n) => n.name), ['lfo', 'drums', 'out']);
  assert.deepEqual(breadcrumbs(drawn, viewport).map((c) => c.path), ['']);
});

// M2.13-3: the inside is drawn in the box grown into a frame instead of in
// the box; 2026-10-22: it opens when opened (pinned), never by the zoom.
test('a composite opens in place when opened, recursively; the zoom alone opens and closes nothing', () => {
  for (const f of [0.01, 0.99, 1.01, 100]) assert.equal(scene({ layoutOf, view: drumsAt(f), viewport }).levels.length, 1, `closed at ${f}`);
  let drawn = scene({ layoutOf, view: drumsAt(1.01), viewport, pinned: DRUMS });
  assert.deepEqual(drawn.levels.map((l) => [l.path, l.level]), [['', ''], ['/drums', 'drums']]);
  const inner = drawn.levels[1];
  // Its inside is drawn readable, in its frame: its box grown where it is.
  assert.ok(inner.transform.k >= READ_K, `inside at ${inner.transform.k}`);
  const frame = drawn.levels[0].layout.nodes.find((n) => n.name === 'drums');
  const box = drums();
  assert.ok(frame.w > box.w && frame.x <= box.x && frame.y <= box.y && frame.x + frame.w >= box.x + box.w && frame.y + frame.h >= box.y + box.h, 'the frame holds the box');
  assert.ok(Math.abs(frame.x + frame.w / 2 - (box.x + box.w / 2)) < 1e-9 && Math.abs(frame.y + frame.h / 2 - (box.y + box.h / 2)) < 1e-9, 'about its centre');
  const shown = screenRect(drawn.levels[0].transform, frame);
  const inside = levelRect(drawn, '/drums');
  assert.ok(inside.x >= shown.x && inside.x + inside.w <= shown.x + shown.w + 1e-9);
  assert.ok(inside.y >= shown.y && inside.y + inside.h <= shown.y + shown.h + 1e-9);
  assert.equal(layoutOf('').nodes.find((n) => n.name === 'drums'), box, 'the stored layout is not changed');

  // Zoomed far out it stays open.
  drawn = scene({ layoutOf, view: drumsAt(0.01), viewport, pinned: DRUMS });
  assert.equal(drawn.levels.length, 2, 'open stays open');

  // Zoomed far in, kick stays closed until opened inside drums.
  assert.deepEqual(scene({ layoutOf, view: kickFocused(), viewport, pinned: DRUMS }).levels.map((l) => l.path), ['', '/drums']);
  const deep = scene({ layoutOf, view: kickFocused(), viewport, pinned: KICK_OPEN });
  assert.deepEqual(deep.levels.map((l) => l.path), ['', '/drums', '/drums/kick']);
  assert.deepEqual(breadcrumbs(deep, viewport).map((c) => c.name), ['', 'drums', 'kick']);
});

test('a double-clicked composite opens whatever its size', () => {
  const view = fit(layoutOf('').bounds, viewport);
  const drawn = scene({ layoutOf, view, viewport, pinned: new Set(['/drums']) });
  assert.deepEqual(drawn.levels.map((l) => l.path), ['', '/drums']);
});

test('only what is on screen is drawn', () => {
  const deep = scene({ layoutOf, view: kickFocused(), viewport, pinned: KICK_OPEN });
  // lfo and out are far off screen at this zoom.
  assert.deepEqual(deep.levels[0].nodes.map((n) => n.name), ['drums']);
  const total = deep.levels.reduce((s, l) => s + l.nodes.length, 0);
  assert.ok(total < 3 + 3 + 3, `${total} nodes drawn`);
  const away = scene({ layoutOf, view: { k: 1, x: 1e6, y: 0 }, viewport });
  assert.deepEqual(away.levels[0].nodes, []);
  assert.deepEqual(away.levels[0].wires, []);
});

test('the breadcrumbs’ levels and parts can be focused', () => {
  const deep = scene({ layoutOf, view: kickFocused(), viewport, pinned: KICK_OPEN });
  const rect = levelRect(deep, '/drums');
  const focused = focus(kickFocused(), rect, viewport);
  // As a breadcrumb does: what is open below it is closed (unpinned), whatever the scale.
  const out = scene({ layoutOf, view: focused, viewport, pinned: DRUMS });
  assert.equal(breadcrumbs(out, viewport).at(-1).path, '/drums', 'the view shows drums’ inside');
  assert.ok(!out.open.has('/drums/kick'), 'kick closed although it was open');
  assert.ok(nodeRect(deep, '/drums/kick'));
  assert.equal(nodeRect(deep, '/nope'), null);
});

test('exported outputs and parameters lead to the plan’s ports', () => {
  assert.equal(planPort(KICK, '/drums.out'), '/drums/kick/dc.out');
  assert.equal(planPort(KICK, '/drums/kick.out'), '/drums/kick/dc.out');
  assert.equal(planPort(KICK, '/drums/kick/osc.out'), '/drums/kick/osc.out');
  assert.equal(planPort(KICK, '/lfo.out'), '/lfo.out');
  assert.equal(planPort(KICK, '/drums.kick_decay', { parameter: true }), '/drums/kick/env.decay_ms');
  assert.equal(planPort(KICK, '/nope.out'), null);
});

test('hit testing finds ports, boxes, wires and the background, innermost first', () => {
  const view = fit(layoutOf('').bounds, viewport);
  const drawn = scene({ layoutOf, view, viewport });
  const top = drawn.levels[0];
  const lfo = top.layout.nodes.find((n) => n.name === 'lfo');
  const port = toScreen(top.transform, lfo.outs[0]);
  assert.deepEqual(hitTest(drawn, { x: port.x + 3, y: port.y }).address, '/lfo.out');
  assert.equal(hitTest(drawn, { x: port.x + 3, y: port.y }).kind, 'port');
  const centre = toScreen(top.transform, { x: lfo.x + lfo.w / 2, y: lfo.y + lfo.h / 2 });
  const box = hitTest(drawn, centre);
  assert.deepEqual([box.kind, box.address], ['instance', '/lfo']);
  const wire = top.layout.wires.find((w) => w.written.from === '/drums.out');
  const [a, b] = wire.points;
  const onWire = toScreen(top.transform, { x: (a.x + b.x) / 2, y: a.y });
  const hit = hitTest(drawn, { x: onWire.x, y: onWire.y + 2 });
  assert.deepEqual([hit.kind, hit.from, hit.to], ['wire', '/drums.out', '/out.l']);
  assert.equal(hitTest(drawn, { x: onWire.x, y: onWire.y + 20 }).kind, 'background');
  assert.equal(hitTest(drawn, { x: -50, y: -50 }).kind, 'background');

  // Opened, a box inside drums wins over drums itself.
  const deep = scene({ layoutOf, view: drumsAt(1.01), viewport, pinned: DRUMS });
  const inner = deep.levels[1];
  const kick = inner.layout.nodes.find((n) => n.name === 'kick');
  const at = toScreen(inner.transform, { x: kick.x + kick.w / 2, y: kick.y + kick.h / 2 });
  assert.equal(hitTest(deep, at).address, '/drums/kick');
});

// auditor-1-004: after a new graph the selection is found again in it, with
// its new module and parameters, or let go when it is gone.
test('a selection is resolved against the new graph, or cleared', () => {
  const before = layout(KICK.top);
  const lfo = before.nodes.find((n) => n.name === 'lfo');
  const selection = { kind: 'instance', path: '', level: '', node: lfo, address: '/lfo' };
  const changed = {
    ...KICK.top,
    instances: KICK.top.instances.map((i) => (i.name === 'lfo' ? { ...i, module: 'std.osc', parameters: [{ name: 'freq', rate: 1 }] } : i)),
  };
  const after = layout(changed);
  const found = reselect(selection, (level) => (level === '' ? after : null));
  assert.equal(found.node.module, 'std.osc');
  assert.deepEqual(found.node.parameters.map((p) => p.name), ['freq']);
  const gone = layout({ ...KICK.top, instances: KICK.top.instances.filter((i) => i.name !== 'lfo') });
  assert.equal(reselect(selection, () => gone), null);
  const port = { kind: 'port', path: '', level: '', node: lfo, port: lfo.outs[0], address: '/lfo.out' };
  assert.equal(reselect(port, () => after).port.name, 'out');
  const wire = before.wires.find((w) => w.written.from === '/drums.out');
  assert.equal(reselect({ kind: 'wire', path: '', level: '', wire }, () => after).wire.written.to, '/out.l');
  assert.equal(reselect({ kind: 'wire', path: '', level: '', wire }, () => ({ ...gone, wires: [] })), null);
  assert.equal(reselect({ ...selection, level: 'nope' }, () => null), null, 'its level is gone');
});

// auditor-1-004: the whole path is checked, not only the leaf's level. Two
// instances share a composite; the selection sits under the first.
test('a selection under a removed or retyped ancestor is cleared', () => {
  const shared = {
    instances: [{ name: 'osc', module: 'std.sine_osc', composite: false, ports: [p('out', 'out')], parameters: [{ name: 'freq', rate: 1 }] }],
    connections: [], inputs: [], outputs: [], parameters: [],
  };
  const other = { ...shared, instances: [{ ...shared.instances[0], name: 'osc', module: 'std.saw' }] };
  const box = (name, module) => ({ name, module, composite: true, ports: [], parameters: [] });
  const graphOf = (instances, composites) => {
    const top = layout({ instances, connections: [] });
    const inside = Object.fromEntries(Object.entries(composites).map(([k, v]) => [k, layout(v)]));
    return (level) => (level === '' ? top : inside[level] ?? null);
  };
  const before = graphOf([box('first', 'voice'), box('second', 'voice')], { voice: shared });
  const osc = before('voice').nodes.find((n) => n.name === 'osc');
  const selection = { kind: 'instance', path: '/first', level: 'voice', node: osc, address: '/first/osc' };
  assert.equal(reselect(selection, before).address, '/first/osc', 'still there');
  // first removed; voice is still used by second.
  assert.equal(reselect(selection, graphOf([box('second', 'voice')], { voice: shared })), null);
  // first retyped to another composite that also holds an osc.
  assert.equal(reselect(selection, graphOf([box('first', 'other'), box('second', 'voice')], { voice: shared, other })), null);
  // first no longer a composite at all.
  const plain = { name: 'first', module: 'std.sine_osc', composite: false, ports: [p('out', 'out')], parameters: [] };
  assert.equal(reselect(selection, graphOf([plain, box('second', 'voice')], { voice: shared })), null);
  // A port under the removed instance goes too.
  const port = { kind: 'port', path: '/first', level: 'voice', node: osc, port: osc.outs[0], address: '/first/osc.out' };
  assert.equal(reselect(port, graphOf([box('second', 'voice')], { voice: shared })), null);
});

// M2.9-1b: re-rooting, deep zoom, culling.

/** A stub node and a stub layout (bounds around its nodes). */
const stubNode = (name, x, y, w, h, module = null) => ({ id: name, name, kind: 'instance', composite: Boolean(module), module: module ?? 'std.x', x, y, w, h, ins: [], outs: [] });
function stub(nodes, wires = []) {
  const xs = nodes.flatMap((n) => [n.x, n.x + n.w]);
  const ys = nodes.flatMap((n) => [n.y, n.y + n.h]);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { nodes, wires, bounds: { x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y } };
}

/** Every drawn level as the schematic hands it to SVG, baked again only as it would be: within the bounds (AC16). */
function bakedInBounds(drawn, bakes, viewport) {
  for (const level of drawn.levels) {
    if ((!level.nodes.length && !level.wires.length) || level.behind) continue;
    let baked = bakes.get(level.path);
    if (!baked || !bakeHolds(baked, level.transform, viewport)) {
      baked = bake(level.transform, viewport);
      for (const node of level.nodes) bakedRect(baked, node);
      bakes.set(level.path, baked);
    }
    const m = bakedMatrix(baked, level.transform);
    assert.ok(m.k >= SVG_MIN_SCALE && m.k <= SVG_MAX_SCALE, `${level.path}: scale ${m.k}`);
    assert.ok(Math.abs(m.x) <= SVG_MAX_TRANSLATE && Math.abs(m.y) <= SVG_MAX_TRANSLATE, `${level.path}: translation ${m.x}, ${m.y}`);
  }
}

// AC16–AC19: ten nested composites, each inside 1e4 times its box, so its
// frame (M2.13-3: GROW_MAX times its box's scale) draws it at about 5e-4;
// zooming in about the innermost one reaches a total scale past 1e40, the
// view re-rooted at the deepest level, every matrix within the bounds.
test('zooming through ten levels to a total scale of 1e40 keeps every matrix small', () => {
  const W = 1.84e6;
  const H = 0.92e6;
  const level = (i) => {
    const corners = [stubNode('a', 0, 0, 200, 100), stubNode('b', W - 200, H - 100, 200, 100)];
    const centre = i < 10 ? stubNode('c', W / 2 - 100, H / 2 - 50, 200, 100, `L${i + 1}`) : stubNode('p', W / 2 - 100, H / 2 - 50, 200, 100);
    return stub([...corners, centre]);
  };
  const layouts = new Map(Array.from({ length: 11 }, (_, i) => [i === 0 ? '' : `L${i}`, level(i)]));
  const layoutOf = (name) => layouts.get(name) ?? null;
  const viewport = { w: 1000, h: 600 };
  const centreOf = (n) => ({ x: n.x + n.w / 2, y: n.y + n.h / 2 });
  let view = fit(layoutOf('').bounds, viewport);
  // Every c opened (2026-10-22: by the person, as a click does; the zoom opens nothing).
  const pinned = new Set(Array.from({ length: 10 }, (_, i) => '/c'.repeat(i + 1)));
  let drawn = null;
  const bakes = new Map();
  for (let step = 0; step < 3000; step++) {
    drawn = scene({ layoutOf, view, viewport, pinned });
    view = drawn.view;
    bakedInBounds(drawn, bakes, viewport);
    if (drawn.levels.length === 11 && drawn.levels[0].transform.k >= 1e40) break;
    // Zoom in about the innermost composite (or the leaf's node).
    const deepest = drawn.levels.at(-1);
    const target = deepest.layout.nodes.find((n) => n.name === 'c' || n.name === 'p');
    view = zoomAt(view, 1.25, toScreen(deepest.transform, centreOf(target)));
  }
  assert.equal(drawn.levels.length, 11, 'ten levels open');
  assert.ok(drawn.levels[0].transform.k >= 1e40, `total scale ${drawn.levels[0].transform.k}`);
  assert.equal(view.path, '/c/c/c/c/c/c/c/c/c/c', 'the view is in the deepest level’s coordinates');
  // Its numbers are the leaf's own, thirty orders below the total. (Before
  // M2.13-3 `view.k < 1e6`: the frames' growth leaves the leaf at about
  // 1.06e7 when the total reaches 1e40, measured.)
  assert.equal(view.k, drawn.levels[10].transform.k, 'the view carries the leaf’s own scale');
  assert.ok(view.k <= drawn.levels[0].transform.k * 1e-30, `and its numbers are small: ${view.k}`);
  // AC17: the parents' other boxes, far off screen, are culled; and a
  // parent that shows only the inside of its open box is not drawn at all.
  for (const parent of drawn.levels.slice(0, 10)) assert.deepEqual(parent.nodes.map((n) => n.name), ['c']);
  assert.ok(drawn.levels.slice(0, 9).every((l) => l.behind), 'the far parents are culled');
  assert.ok(!drawn.levels[10].behind, 'the leaf is drawn');
  // The leaf's node is on screen where hit testing finds it.
  const leaf = drawn.levels[10];
  const p = leaf.layout.nodes.find((n) => n.name === 'p');
  const at = toScreen(leaf.transform, centreOf(p));
  assert.ok(at.x > 0 && at.x < viewport.w && at.y > 0 && at.y < viewport.h);
  assert.equal(hitTest(drawn, at).address, '/c/c/c/c/c/c/c/c/c/c/p');
});

// M2.13-3 (AC5, the owner's intent): ten nested composites of an ordinary
// size, reached by zooming about the innermost one, each opened by a click
// once its box is on screen (2026-10-22: the zoom alone opens nothing), the
// breadcrumbs the open chain, within 400 wheel steps.
test('ten nested composites of ordinary size are reached by zooming and clicking each open', () => {
  const level = (i) => {
    const nodes = [stubNode('a', 0, 0, 120, 90), stubNode('b', 880, 410, 120, 90)];
    nodes.push(i < 10 ? stubNode('c', 440, 205, 120, 90, `L${i + 1}`) : stubNode('p', 440, 205, 120, 90));
    return stub(nodes);
  };
  const layouts = new Map(Array.from({ length: 11 }, (_, i) => [i === 0 ? '' : `L${i}`, level(i)]));
  const layoutOf = (name) => layouts.get(name) ?? null;
  const centreOf = (n) => ({ x: n.x + n.w / 2, y: n.y + n.h / 2 });
  let view = fit(layoutOf('').bounds, viewport);
  let pinned = new Set();
  let drawn = null;
  let steps = 0;
  const bakes = new Map();
  for (; steps < 400; steps++) {
    drawn = scene({ layoutOf, view, viewport, pinned });
    view = drawn.view;
    bakedInBounds(drawn, bakes, viewport);
    if (drawn.levels.length === 11) break;
    const deepest = drawn.levels.at(-1);
    const target = deepest.layout.nodes.find((n) => n.name === 'c');
    // Its box clicked where it is on screen: it opens (and stays open as the zoom goes on).
    const at = toScreen(deepest.transform, centreOf(target));
    if (hitTest(drawn, at).address === `${deepest.path}/c` && target.w * deepest.transform.k >= 40) {
      pinned = pinOne(pinned, `${deepest.path}/c`);
      continue;
    }
    view = zoomAt(view, 1.25, at);
  }
  assert.equal(drawn.levels.length, 11, `ten levels open after ${steps} steps`);
  assert.deepEqual(breadcrumbs(drawn).map((c) => c.name), ['', 'c', 'c', 'c', 'c', 'c', 'c', 'c', 'c', 'c', 'c']);
  assert.equal(view.path, '/c/c/c/c/c/c/c/c/c/c');
  // The innermost die is hit once it is drawn (ADR 0047 §8: its text at its threshold): zoomed on until it is.
  let leaf = drawn.levels[10];
  const p = leaf.layout.nodes.find((n) => n.name === 'p');
  for (let i = 0; i < 100 && belowThreshold(leaf.transform.k); i++) {
    drawn = scene({ layoutOf, view: zoomAt(view, 1.25, toScreen(leaf.transform, centreOf(p))), viewport, pinned });
    view = drawn.view;
    leaf = drawn.levels[10];
  }
  const at = toScreen(leaf.transform, centreOf(p));
  assert.equal(hitTest(drawn, at).address, '/c/c/c/c/c/c/c/c/c/c/p');
});

test('the breadcrumbs are the open levels; inside an open frame only the frame is hit', () => {
  const drawn = scene({ layoutOf, view: drumsAt(1.01), viewport, pinned: DRUMS });
  assert.deepEqual(breadcrumbs(drawn).map((c) => c.path), ['', '/drums']);
  // A neighbour under the grown frame: a point over it is the frame's.
  const top = stub([stubNode('c', 0, 0, 120, 90, 'inner'), stubNode('n', 130, 0, 120, 90)]);
  const inner = stub([stubNode('x', 0, 0, 120, 90), stubNode('y', 880, 410, 120, 90)]);
  const stubs = (name) => (name === '' ? top : name === 'inner' ? inner : null);
  const c = top.nodes[0];
  const k = (READ_K / linkScale(c, inner.bounds)) * 1.01;
  const shown = scene({ layoutOf: stubs, view: { k, ox: 60, oy: 45, x: 500, y: 300 }, viewport, pinned: new Set(['/c']) });
  assert.deepEqual(shown.levels.map((l) => l.path), ['', '/c']);
  const frame = shown.levels[0].layout.nodes.find((n) => n.name === 'c');
  const n = top.nodes[1];
  assert.ok(frame.x + frame.w > n.x + n.w / 2, 'the frame covers n’s middle');
  const over = toScreen(shown.levels[0].transform, { x: n.x + n.w / 2, y: n.y + n.h / 2 });
  assert.equal(hitTest(shown, over).address, '/c');
  assert.equal(hitTest(scene({ layoutOf: stubs, view: { k: k * 0.5, ox: 60, oy: 45, x: 500, y: 300 }, viewport }), toScreen({ k: k * 0.5, ox: 60, oy: 45, x: 500, y: 300 }, { x: n.x + n.w / 2, y: n.y + n.h / 2 })).address, '/n', 'closed, n is hit');
});

// auditor-2-far-parent-cull-and-translation-clamp: a voice at the bottom of
// the 4096-voice top level (y ≈ 4.7e5) stays drawn with its parent, at any
// scale, and panning out of it re-roots the view without moving anything.
test('a parent far from its origin stays drawn, and re-rooting moves nothing', () => {
  const voice = stubNode('voice_4095', 1000, 466830, 117, 90, 'voice');
  const above = stubNode('voice_4094', 1000, 466830 - 114, 117, 90, 'voice');
  const below = stubNode('mix', 1000, 466830 + 114, 117, 90);
  const top = stub([stubNode('transport', 0, 0, 117, 90), above, voice, below]);
  const inner = stub([stubNode('osc', 0, 0, 100, 60), stubNode('env', 200, 0, 100, 60)]);
  const layoutOf = (name) => (name === '' ? top : name === 'voice' ? inner : null);
  const viewport = { w: 1000, h: 600 };
  const edge = { x: voice.x, y: voice.y + voice.h / 2 };
  for (const k of [4, 21, 1e3, 1e6, 1e9]) {
    const view = { k, ox: edge.x, oy: edge.y, x: 500, y: 300 };
    // Opened (as a click opens it) where it is drawn readable.
    const readable = k * linkScale(voice, inner.bounds) >= READ_K;
    const pinned = readable ? new Set(['/voice_4095']) : new Set();
    const drawn = scene({ layoutOf, view, viewport, pinned });
    const parent = drawn.levels[0];
    const names = parent.nodes.map((n) => n.name);
    assert.ok(names.includes('voice_4095'), `k ${k}: the voice's box is drawn`);
    if (k === 4) assert.ok(names.includes('voice_4094') && names.includes('mix'), 'and its neighbours');
    bakedInBounds(drawn, new Map(), viewport);
    // Its left edge, as baked and drawn, is where the view puts it.
    const baked = bake(parent.transform, viewport);
    const box = bakedRect(baked, voice);
    const matrix = bakedMatrix(baked, parent.transform);
    assert.ok(Math.abs(box.x * matrix.k + matrix.x - 500) < 0.01, `k ${k}: edge at ${box.x * matrix.k + matrix.x}`);
    if (readable) assert.equal(drawn.view.path, '/voice_4095', `k ${k}: re-rooted in the voice`);
    // Panned 100 px right past its frame's left edge (M2.13-3: the box grows
    // into a frame about its centre, so its edge is left of the box's), the
    // centre leaves the voice: re-rooted at the top.
    const margin = (voice.x - grownNode(voice, inner.bounds).x) * k;
    const out = scene({ layoutOf, view: pan(drawn.view, margin + 100, 0), viewport, pinned });
    assert.equal(out.view.path, '', `k ${k}`);
    const now = toScreen(out.levels[0].transform, edge);
    const to = 600 + margin;
    assert.ok(Math.abs(now.x - to) < 0.01 && Math.abs(now.y - 300) < 0.01, `k ${k}: the edge moved with the pan only (${now.x})`);
  }
});

// AC3: a wire shorter than MIN_PX on screen is culled, as a node is.
test('wires shorter than MIN_PX on screen are culled', () => {
  const wire = { id: 0, shape: 'signal', points: [{ x: 0, y: 0 }, { x: 10, y: 0 }] };
  const flat = { nodes: [], wires: [wire], bounds: { x: 0, y: 0, w: 10, h: 1 } };
  const viewport = { w: 100, h: 100 };
  const shown = (k) => scene({ layoutOf: () => flat, view: { k, ox: 5, oy: 0, x: 50, y: 50 }, viewport }).levels[0].wires.length;
  assert.equal(10 * 0.1 < MIN_PX, true);
  assert.equal(shown(0.1), 0, '1 px: culled');
  assert.equal(shown(0.3), 1, '3 px: drawn');
});

// Review 2026-10-31 (P2): the board drew every wire, even with the board wholly off screen. Its wires are culled by their
// segments against the view (every wire crossing the view is drawn, whatever its length and the zoom); none off it.
test('a board culls its wires by their segments against the view: off screen none, a wire crossing the view with both ends off it drawn', () => {
  const across = { id: 'across', shape: 'signal', points: [{ x: -500, y: 300 }, { x: 1500, y: 300 }] };
  const away = { id: 'away', shape: 'signal', points: [{ x: 2000, y: 2000 }, { x: 2500, y: 2000 }] };
  // Its box covers the view, its segments pass outside it (above, then right of it).
  const around = { id: 'around', shape: 'events', points: [{ x: -100, y: -100 }, { x: 1200, y: -100 }, { x: 1200, y: 900 }] };
  const tiny = { id: 'tiny', shape: 'signal', points: [{ x: 500, y: 500 }, { x: 500.5, y: 500 }] };
  const board = { board: {}, nodes: [], wires: [across, away, around, tiny], bounds: { x: -500, y: -100, w: 3000, h: 2100 } };
  const viewport = { w: 1000, h: 600 };
  const ids = (view) => scene({ layoutOf: (level) => (level === '' ? board : null), view, viewport }).levels[0].wires.map((w) => w.id);
  assert.deepEqual(ids({ k: 1, x: 0, y: 0 }), ['across', 'tiny'], 'the wire across the view drawn, its ends off it; a short one drawn at any zoom');
  assert.deepEqual(ids({ k: 1, x: 10000, y: 10000 }), [], 'the board off screen: no wire');
  assert.deepEqual(ids({ k: 0.01, x: 500, y: 300 }), ['across', 'away', 'around', 'tiny'], 'zoomed out over the whole board: every wire');
});

// A new layout keeps the view in the deepest level whose chain did not change.
test('a new layout rebases the view on what did not move', () => {
  const view = scene({ layoutOf, view: kickFocused(), viewport, pinned: KICK_OPEN }).view;
  assert.equal(view.path, '/drums/kick');
  const top = layoutOf('');
  const moved = (name, dx) => ({ ...top, nodes: top.nodes.map((n) => (n.name === name ? { ...n, x: n.x + dx } : n)) });
  // lfo moved: drums' box is where it was, so the view stays in kick.
  const lfoMoved = moved('lfo', 40);
  assert.deepEqual(rebase(view, layoutOf, (l) => (l === '' ? lfoMoved : layoutOf(l))), view);
  // drums moved: the view goes to the top level, which did not move.
  const drumsMoved = moved('drums', 40);
  const rebased = rebase(view, layoutOf, (l) => (l === '' ? drumsMoved : layoutOf(l)));
  assert.equal(rebased.path, '');
  const before = scene({ layoutOf, view, viewport, pinned: KICK_OPEN }).levels[0].transform;
  const lfo = top.nodes.find((n) => n.name === 'lfo');
  const a = toScreen(before, lfo);
  const b = toScreen(rebased, lfo);
  assert.ok(Math.abs(a.x - b.x) < 1e-6 && Math.abs(a.y - b.y) < 1e-6, 'lfo stays where it was');
  // The kick inside moved with drums' box.
  assert.ok(inside(drumsMoved.nodes.find((n) => n.name === 'drums'), layoutOf('drums').bounds).x !== inside(drums(), layoutOf('drums').bounds).x);
});

test('the links across an open composite’s edge: each box pin to its frame pin, in the inside’s units (2026-10-20)', () => {
  const box = drums();
  const inner = layoutOf('drums');
  const links = boundaryLinks(box, inner);
  const frameIns = inner.nodes.find((n) => n.kind === 'input');
  const frameOuts = inner.nodes.find((n) => n.kind === 'output');
  assert.equal(links.length, (frameIns?.outs.length ?? 0) + (frameOuts?.ins.length ?? 0));
  const t = inside(box, inner.bounds);
  for (const l of links) {
    const boxPin = (l.direction === 'in' ? box.ins : box.outs).find((p) => p.name === l.name);
    const framePin = (l.direction === 'in' ? frameIns.outs : frameOuts.ins).find((p) => p.name === l.name);
    // The box pin's end, taken back to the parent's units, is the box pin; the other end the frame pin.
    const boxEnd = l.direction === 'in' ? l.from : l.to;
    const frameEnd = l.direction === 'in' ? l.to : l.from;
    assert.ok(Math.abs(boxEnd.x * t.k + t.x - boxPin.x) < 1e-6 && Math.abs(boxEnd.y * t.k + t.y - boxPin.y) < 1e-6, l.name);
    assert.deepEqual(frameEnd, { x: framePin.x, y: framePin.y });
    // Upstream to downstream: an input's box pin is left of its frame pin, an output's right of it.
    assert.ok(l.from.x < l.to.x, `${l.name}: ${l.from.x} < ${l.to.x}`);
  }
  // A grouped box pin (its groups collapsed) starts at its group's pin.
  const anchors = new Map([[links[0].name, { x: box.x, y: box.y + 100 }]]);
  const moved = boundaryLinks(box, inner, anchors)[0];
  assert.ok(Math.abs(moved.from.y * t.k + t.y - (box.y + 100)) < 1e-6);
  assert.deepEqual(boundaryLinks(null, inner), []);
});
