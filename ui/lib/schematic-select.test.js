// The schematic's selection of several parts (docs/ui-controls.md
// "Schematic"): Shift- or ⌘-click toggling, the marquee's take, ⌘A on the
// level shown, the selection found again after a `graph` push, what a
// group's drag moves and what Delete writes, one patch per level.

import test from 'node:test';
import assert from 'node:assert/strict';
import { layout } from './schematic-layout.js';
import { scene } from './schematic-scene.js';
import { READ_K, linkScale } from './schematic-open.js';
import { MIN_PORT_PX, hitTest, instanceHit, portHit, reselect, wireHit } from './schematic-hit.js';
import { MESSAGE_LIMIT, allOf, deletePlan, dragGroup, has, hitKey, inside, messageBytes, moveMessages, reselectAll, toggled, union, unique } from './schematic-select.js';
import { fit, screenRect, toScreen } from './schematic-view.js';
import { KICK } from './schematic-scene.test.js';

const layouts = new Map();
const layoutOf = (level) => {
  if (!layouts.has(level)) layouts.set(level, layout(level === '' ? KICK.top : KICK.composites[level]));
  return layouts.get(level);
};
const viewport = { w: 1000, h: 600 };
const TOP = { path: '', level: '' };
const nodeOf = (level, name) => layoutOf(level).nodes.find((n) => n.name === name);
const box = (name) => instanceHit({ ...TOP }, nodeOf('', name));
const wireOf = (from) => wireHit({ ...TOP }, layoutOf('').wires.find((w) => w.written.from === from));
const keys = (list) => list.map(hitKey);

test('each part has its own key: an instance, a pin by direction, a wire by its level and ends', () => {
  const lfo = nodeOf('', 'lfo');
  assert.equal(hitKey(box('lfo')), 'instance /lfo');
  assert.equal(hitKey(portHit(TOP, lfo, lfo.outs[0])), 'port /lfo.out out');
  assert.equal(hitKey(wireOf('/drums.out')), 'wire  /drums.out /out.l');
  assert.equal(hitKey({ kind: 'background' }), '');
  assert.equal(hitKey(null), '');
});

test('Shift- or ⌘-click adds a part, a second takes it out; the background changes nothing', () => {
  let selected = [];
  selected = toggled(selected, box('lfo'));
  selected = toggled(selected, wireOf('/drums.out'));
  selected = toggled(selected, box('out'));
  assert.deepEqual(keys(selected), ['instance /lfo', 'wire  /drums.out /out.l', 'instance /out']);
  // The same part found by another click (a new hit object) is taken out.
  selected = toggled(selected, box('lfo'));
  assert.deepEqual(keys(selected), ['wire  /drums.out /out.l', 'instance /out']);
  assert.equal(toggled(selected, { kind: 'background', path: '', level: '' }), selected);
  assert.ok(has(selected, box('out')));
  assert.ok(!has(selected, box('lfo')));
  assert.deepEqual(keys(unique([box('out'), box('out'), { kind: 'background' }, box('lfo')])), ['instance /out', 'instance /lfo']);
  assert.deepEqual(keys(union([box('out')], [box('lfo'), box('out')])), ['instance /out', 'instance /lfo']);
});

test('the marquee takes the boxes wholly inside it and the connections between them', () => {
  // Fitted as Fit does: drums closed.
  const view = fit(layoutOf('').bounds, viewport);
  const drawn = scene({ layoutOf, view, viewport });
  const top = drawn.levels[0];
  const rect = (names) => {
    const rects = names.map((n) => screenRect(top.transform, nodeOf('', n)));
    const x0 = Math.min(...rects.map((r) => r.x)) - 2;
    const y0 = Math.min(...rects.map((r) => r.y)) - 2;
    const x1 = Math.max(...rects.map((r) => r.x + r.w)) + 2;
    const y1 = Math.max(...rects.map((r) => r.y + r.h)) + 2;
    return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
  };
  const both = inside(drawn, rect(['drums', 'out']));
  assert.ok(keys(both).includes('instance /drums') && keys(both).includes('instance /out'));
  assert.ok(keys(both).includes('wire  /drums.out /out.l'), 'the wire between two boxes taken');
  assert.ok(!keys(both).some((k) => k.startsWith('wire  /lfo')), 'not a wire with one end outside');
  // Drawn from the other corner: the same take.
  const r = rect(['drums', 'out']);
  assert.deepEqual(keys(inside(drawn, { x: r.x + r.w, y: r.y + r.h, w: -r.w, h: -r.h })), keys(both));
  // A box cut by the edge is not taken.
  const lfo = screenRect(top.transform, nodeOf('', 'lfo'));
  assert.deepEqual(inside(drawn, { x: lfo.x + 1, y: lfo.y - 2, w: lfo.w, h: lfo.h + 4 }), []);
});

test('the marquee does not take the boxes inside an open composite it takes', () => {
  // drums opened in place: its inside is drawn, and kick inside it.
  const drums = nodeOf('', 'drums');
  // Opened (pinned, as a click opens it; 2026-10-22), its inside readable.
  const k = (READ_K / linkScale(drums, layoutOf('drums').bounds)) * 1.1;
  const view = { k, x: viewport.w / 2 - (drums.x + drums.w / 2) * k, y: viewport.h / 2 - (drums.y + drums.h / 2) * k };
  const drawn = scene({ layoutOf, view, viewport, pinned: new Set(['/drums']) });
  assert.ok(drawn.levels.some((l) => l.path === '/drums'), 'drums is open');
  const all = inside(drawn, { x: -1e6, y: -1e6, w: 2e6, h: 2e6 });
  assert.ok(keys(all).includes('instance /drums'));
  assert.ok(!keys(all).some((key) => key.startsWith('instance /drums/')), 'its inside is deleted with it anyway');
  // Only the inside's box in the rectangle: that one is taken.
  const inner = drawn.levels.find((l) => l.path === '/drums');
  const kick = screenRect(inner.transform, inner.layout.nodes.find((n) => n.name === 'kick'));
  assert.deepEqual(keys(inside(drawn, { x: kick.x - 1, y: kick.y - 1, w: kick.w + 2, h: kick.h + 2 })), ['instance /drums/kick']);
});

test('⌘A takes every instance and connection of the level shown, not its exports', () => {
  const top = { path: '', level: '', layout: layoutOf('') };
  assert.deepEqual(keys(allOf(top)), ['instance /lfo', 'instance /drums', 'instance /out', 'wire  /lfo.out /drums.kick_decay', 'wire  /drums.out /out.l']);
  const kick = { path: '/drums/kick', level: 'kick', layout: layoutOf('kick') };
  assert.deepEqual(keys(allOf(kick)), ['instance /drums/kick/osc', 'instance /drums/kick/env', 'instance /drums/kick/dc', 'wire /drums/kick osc.out dc.in']);
  assert.deepEqual(allOf(undefined), []);
});

test('after a graph push the selection is found again as one part at a time would be', () => {
  // Every kind, at every depth, against the same and a changed graph.
  const drums = nodeOf('', 'drums');
  const kickEntry = { path: '/drums/kick', level: 'kick' };
  const all = [
    ...allOf({ ...TOP, layout: layoutOf('') }),
    ...allOf({ ...kickEntry, layout: layoutOf('kick') }),
    portHit(TOP, drums, drums.outs[0]),
    ...layoutOf('kick').wires.map((w) => wireHit(kickEntry, w)),
    { ...instanceHit({ path: '/gone', level: 'kick' }, nodeOf('kick', 'osc')) },
  ];
  const changed = { ...KICK.composites, kick: { ...KICK.composites.kick, instances: KICK.composites.kick.instances.filter((i) => i.name !== 'env') } };
  for (const source of [KICK.composites, changed]) {
    const fresh = new Map();
    const now = (level) => {
      if (!fresh.has(level)) fresh.set(level, level === '' ? layout(KICK.top) : source[level] ? layout(source[level]) : null);
      return fresh.get(level);
    };
    assert.deepEqual(reselectAll(all, now), unique(all.map((s) => reselect(s, now)).filter(Boolean)));
  }
});

test('after a graph push the selection is found again; what went is let go', () => {
  const lfo = nodeOf('', 'lfo');
  const selected = [box('lfo'), box('out'), wireOf('/drums.out'), portHit(TOP, lfo, lfo.outs[0])];
  const without = layout({ ...KICK.top, instances: KICK.top.instances.filter((i) => i.name !== 'lfo'), connections: KICK.top.connections.filter((c) => !c.from.startsWith('/lfo')) });
  const again = reselectAll(selected, (level) => (level === '' ? without : layoutOf(level)));
  assert.deepEqual(keys(again), ['instance /out', 'wire  /drums.out /out.l']);
  assert.equal(again[0].node, without.nodes.find((n) => n.name === 'out'), 'with the new layout’s node');
});

test('a group’s drag moves the selected boxes of the dragged one’s level, it first', () => {
  const inner = { path: '/drums/kick', level: 'kick' };
  const osc = instanceHit(inner, nodeOf('kick', 'osc'));
  const selected = [box('lfo'), osc, box('out'), wireOf('/drums.out')];
  assert.deepEqual(dragGroup(selected, box('out')), ['out', 'lfo']);
  assert.deepEqual(dragGroup(selected, osc), ['osc']);
  assert.deepEqual(dragGroup(selected, box('drums')), ['drums'], 'a box not selected moves alone');
  assert.deepEqual(dragGroup(selected, wireOf('/drums.out')), []);
});

test('Delete writes one patch per level, connections an instance takes with it left out', () => {
  const inner = { path: '/drums/kick', level: 'kick' };
  const osc = instanceHit(inner, nodeOf('kick', 'osc'));
  const oscDc = wireHit(inner, layoutOf('kick').wires.find((w) => w.written.from === 'osc.out'));
  const exported = wireHit(inner, layoutOf('kick').wires.find((w) => w.kind === 'export'));
  const lfo = nodeOf('', 'lfo');
  const selected = [box('lfo'), wireOf('/lfo.out'), wireOf('/drums.out'), oscDc, osc, exported, portHit(TOP, lfo, lfo.outs[0])];
  const plan = deletePlan(selected);
  assert.deepEqual(plan.patches.map(({ level, ops }) => ({ level, ops })), [
    { level: '', ops: [{ op: 'instance.remove', name: 'lfo' }, { op: 'connection.remove', from: 'drums.out', to: 'out.l' }] },
    { level: 'kick', ops: [{ op: 'instance.remove', name: 'osc' }] },
  ]);
  assert.equal(plan.skipped, 2, 'the export and the pin');
  assert.equal(plan.deleted, 3);
  assert.deepEqual(plan.tooLarge, []);
  // The same composite's instance selected in two places is removed once.
  const twice = deletePlan([osc, { ...osc, path: '/drums2/kick', address: '/drums2/kick/osc' }]);
  assert.deepEqual(twice.patches.map((p) => p.ops), [[{ op: 'instance.remove', name: 'osc' }]]);
  assert.deepEqual(deletePlan([]), { patches: [], skipped: 0, deleted: 0, tooLarge: [] });
  // What lies inside a removed instance (its open inside) goes with it: not written to its composite's file.
  const drums = box('drums');
  const within = deletePlan([drums, { ...osc, path: '/drums/kick', address: '/drums/kick/osc' }, oscDc, box('out')]);
  assert.deepEqual(within.patches.map((p) => [p.level, p.ops.map((o) => o.name)]), [['', ['drums', 'out']]]);
  assert.equal(within.skipped, 0, 'nor counted as left');
});

test('a Delete larger than a message is refused whole: one patch is one undo', () => {
  const many = Array.from({ length: 3000 }, (_, i) => ({ kind: 'instance', path: '', level: '', address: `/box_${i}`, node: { kind: 'instance', name: `box_${i}` } }));
  const plan = deletePlan(many);
  assert.equal(plan.patches.length, 1);
  assert.ok(plan.patches[0].bytes > MESSAGE_LIMIT);
  assert.deepEqual(plan.tooLarge, plan.patches);
  assert.deepEqual(deletePlan(many.slice(0, 100)).tooLarge, []);
  assert.equal(deletePlan(many.slice(0, 100), 1000).tooLarge.length, 1, 'the limit is a parameter');
});

test('a group’s move goes as few messages as fit the limit, in order, none over it', () => {
  assert.deepEqual(moveMessages('', []), []);
  assert.deepEqual(moveMessages('', [{ instance: 'a', x: 1, y: 2 }]), [{ type: 'schematic.move', level: '', instance: 'a', x: 1, y: 2 }]);
  assert.deepEqual(moveMessages('kick', [{ instance: 'a', x: 1, y: 2 }, { instance: 'b', x: 3, y: 4 }]), [
    { type: 'schematic.move', level: 'kick', moves: [{ instance: 'a', x: 1, y: 2 }, { instance: 'b', x: 3, y: 4 }] },
  ]);
  const moves = Array.from({ length: 4096 }, (_, i) => ({ instance: `voice_${i}`, x: 100000 + i, y: -200000 - i }));
  const messages = moveMessages('', moves);
  assert.ok(messages.length > 1, `${messages.length} messages`);
  for (const m of messages) assert.ok(messageBytes(m) <= MESSAGE_LIMIT, `${messageBytes(m)} bytes`);
  assert.deepEqual(messages.flatMap((m) => m.moves ?? [m]).map((m) => m.instance), moves.map((m) => m.instance), 'every move once, in order');
  // A small limit: still every move, one per message at worst, each in the plain form.
  const tiny = moveMessages('', moves.slice(0, 3), 10);
  assert.deepEqual(tiny.map((m) => m.instance), ['voice_0', 'voice_1', 'voice_2']);
});

test('pins are hit within 9 px, the nearest of those in reach', () => {
  // Zoomed out until two pins are 12 px apart: their 9 px reaches overlap.
  const view = { k: 0.75, x: 20, y: 20 };
  const drawn = scene({ layoutOf, view, viewport: { w: 2000, h: 2000 } });
  const top = drawn.levels[0];
  const out = nodeOf('', 'out');
  const [l, r] = out.ins.map((p) => toScreen(top.transform, p));
  assert.equal(hitTest(drawn, { x: l.x - 8, y: l.y }).address, '/out.l');
  // Between two pins, the nearer one.
  const spacing = r.y - l.y;
  assert.ok(spacing < 18, 'the pins are closer than two radii here');
  assert.equal(hitTest(drawn, { x: l.x, y: l.y + spacing * 0.4 }).address, '/out.l');
  assert.equal(hitTest(drawn, { x: l.x, y: l.y + spacing * 0.6 }).address, '/out.r');
});

test('zoomed out, a box keeps its body: a pin reaches no more than a quarter of its box’s height, and never less than MIN_PORT_PX (ADR 0047 phase 4)', () => {
  const k = 0.2;
  const view = { k, x: 20, y: 20 };
  const drawn = scene({ layoutOf, view, viewport: { w: 2000, h: 2000 } });
  const top = drawn.levels[0];
  const lfo = nodeOf('', 'lfo');
  const pin = toScreen(top.transform, lfo.outs[0]);
  const reach = Math.max(MIN_PORT_PX, (lfo.h * k) / 4);
  assert.ok(reach < 9);
  assert.equal(hitTest(drawn, { x: pin.x - reach * 0.9, y: pin.y }).kind, 'port');
  const body = hitTest(drawn, { x: pin.x - reach * 1.2, y: pin.y });
  assert.deepEqual([body.kind, body.address], ['instance', '/lfo'], 'just past it: the box');
});

test('a composite’s frame pin is hit (ADR 0030 §1): the composite instance’s port, found again after a push', () => {
  const view = fit(layoutOf('').bounds, { w: 2000, h: 2000 });
  const drawn = scene({ layoutOf, view, viewport: { w: 2000, h: 2000 }, pinned: new Set(['/drums']) });
  const inner = drawn.levels.find((l) => l.path === '/drums');
  const frame = inner.layout.nodes.find((n) => n.id === '@inputs');
  const pin = frame.outs[0];
  const at = toScreen(inner.transform, pin);
  const hit = hitTest(drawn, at);
  assert.deepEqual([hit.kind, hit.address, hit.node.id, hit.port.direction], ['port', `/drums.${pin.name}`, '@inputs', 'out']);
  assert.equal(hitKey(hit), `port /drums.${pin.name} out`);
  // A frame's box is not a part: beside its pins, the inside's background (or what is behind).
  const beside = hitTest(drawn, toScreen(inner.transform, { x: frame.x + 4, y: frame.y + frame.h - 4 }));
  assert.notEqual(beside.node?.id, '@inputs');
  // Found again in a new layout, by its frame.
  layouts.clear();
  const again = reselect(hit, layoutOf);
  assert.equal(again.node.id, '@inputs');
  assert.notEqual(again.node, hit.node);
  assert.deepEqual(keys(reselectAll([hit], layoutOf)), [hitKey(hit)]);
});
