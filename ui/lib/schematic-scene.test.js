import test from 'node:test';
import assert from 'node:assert/strict';
import { layout } from './schematic-layout.js';
import { CLOSE_PX, OPEN_PX, breadcrumbs, levelRect, nodeRect, planPort, scene } from './schematic-scene.js';
import { hitTest, reselect } from './schematic-hit.js';
import { fit, focus, screenRect, toScreen } from './schematic-view.js';

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

/** The view that makes drums `width` pixels wide, centred. */
function drumsAt(width) {
  const node = drums();
  const k = width / node.w;
  return { k, x: viewport.w / 2 - (node.x + node.w / 2) * k, y: viewport.h / 2 - (node.y + node.h / 2) * k };
}

/** Opened drums, then the view focused on kick inside it, as a double-click does. */
function kickFocused() {
  const view = drumsAt(OPEN_PX + 1);
  return focus(view, nodeRect(scene({ layoutOf, view, viewport }), '/drums/kick'), viewport);
}

test('fitted, only the top level is drawn: composites are closed boxes', () => {
  const view = fit(layoutOf('').bounds, viewport);
  const drawn = scene({ layoutOf, view, viewport });
  assert.equal(drawn.levels.length, 1);
  assert.deepEqual(drawn.levels[0].nodes.map((n) => n.name), ['lfo', 'drums', 'out']);
  assert.deepEqual(breadcrumbs(drawn, viewport).map((c) => c.path), ['']);
});

test('a composite opens in place past the threshold, recursively, and closes with hysteresis', () => {
  let drawn = scene({ layoutOf, view: drumsAt(OPEN_PX + 1), viewport });
  assert.deepEqual(drawn.levels.map((l) => [l.path, l.level]), [['', ''], ['/drums', 'drums']]);
  const inner = drawn.levels[1];
  // Its inside is scaled into its box.
  const box = screenRect(drawn.levels[0].transform, drums());
  const inside = levelRect(drawn, '/drums');
  assert.ok(inside.x >= box.x && inside.x + inside.w <= box.x + box.w + 1e-9);
  assert.ok(inside.y >= box.y && inside.y + inside.h <= box.y + box.h + 1e-9);
  assert.ok(inner.transform.k < drawn.levels[0].transform.k);

  // Between the two thresholds it stays as it was.
  const between = drumsAt((OPEN_PX + CLOSE_PX) / 2);
  assert.equal(scene({ layoutOf, view: between, viewport }).levels.length, 1, 'closed stays closed');
  drawn = scene({ layoutOf, view: between, viewport, open: drawn.open });
  assert.equal(drawn.levels.length, 2, 'open stays open');
  assert.equal(scene({ layoutOf, view: drumsAt(CLOSE_PX - 1), viewport, open: drawn.open }).levels.length, 1, 'closes below CLOSE_PX');

  // Zoomed far in, kick opens inside drums.
  const deep = scene({ layoutOf, view: kickFocused(), viewport, open: new Set(['/drums']) });
  assert.deepEqual(deep.levels.map((l) => l.path), ['', '/drums', '/drums/kick']);
  assert.deepEqual(breadcrumbs(deep, viewport).map((c) => c.name), ['', 'drums', 'kick']);
});

test('a double-clicked composite opens whatever its size', () => {
  const view = fit(layoutOf('').bounds, viewport);
  const drawn = scene({ layoutOf, view, viewport, pinned: new Set(['/drums']) });
  assert.deepEqual(drawn.levels.map((l) => l.path), ['', '/drums']);
});

test('only what is on screen is drawn', () => {
  const deep = scene({ layoutOf, view: kickFocused(), viewport, open: new Set(['/drums']) });
  // lfo and out are far off screen at this zoom.
  assert.deepEqual(deep.levels[0].nodes.map((n) => n.name), ['drums']);
  const total = deep.levels.reduce((s, l) => s + l.nodes.length, 0);
  assert.ok(total < 3 + 3 + 3, `${total} nodes drawn`);
  const away = scene({ layoutOf, view: { k: 1, x: 1e6, y: 0 }, viewport });
  assert.deepEqual(away.levels[0].nodes, []);
  assert.deepEqual(away.levels[0].wires, []);
});

test('the breadcrumbs’ levels and parts can be focused', () => {
  const deep = scene({ layoutOf, view: kickFocused(), viewport, open: new Set(['/drums']) });
  const rect = levelRect(deep, '/drums');
  const back = focus(kickFocused(), rect, viewport);
  const out = scene({ layoutOf, view: back, viewport, open: deep.open });
  assert.equal(breadcrumbs(out, viewport).at(-1).path, '/drums', 'the view shows drums’ inside');
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
  const deep = scene({ layoutOf, view: drumsAt(OPEN_PX + 1), viewport });
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
