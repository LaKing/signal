// The schematic's sidebar (docs/ui-controls.md "Schematic"): what it shows
// for nothing selected, an instance, a wire, a pin and several parts; its
// links select what they name; its actions are the context menu's.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { layout } from './schematic-layout.js';
import { instanceHit, portHit, wireHit } from './schematic-hit.js';
import { hitKey } from './schematic-select.js';
import { EDIT_OFF } from './schematic-menu.js';
import { LEGEND, MEMBERS_SHOWN, acrossOf, TRACE_DEPTHS, TRACE_DIRECTIONS, addressesText, connectionsOf, sidebar, tooLargeText, traceSection } from './schematic-sidebar.js';
import { trace } from './schematic-trace.js';
import { GRAPH } from './schematic-trace.test.js';
import { KICK } from './schematic-scene.test.js';
import { moduleInfo } from './schematic-palette.js';

const layouts = new Map();
const layoutOf = (level) => {
  if (!layouts.has(level)) layouts.set(level, level === '' ? layout(KICK.top) : KICK.composites[level] ? layout(KICK.composites[level]) : null);
  return layouts.get(level);
};
const TOP = { path: '', level: '' };
const nodeOf = (level, name) => layoutOf(level).nodes.find((n) => n.name === name);
const box = (name) => instanceHit(TOP, nodeOf('', name));
const wireOf = (from) => wireHit(TOP, layoutOf('').wires.find((w) => w.written.from === from));
const field = (model, label) => model.fields.find(([l]) => l === label)?.[1];
const ids = (model) => model.actions.map((a) => a.id);
const action = (model, id) => model.actions.find((a) => a.id === id);
const DESCRIBE = {
  playing: true,
  controls: [{ address: '/out.gain', instance: 'out', parameter: 'gain', module: 'std.audio_out', unit: null, min: 0, max: 4, default: 1, base: 0.75, inFile: true }],
  instances: [],
};
const context = (extra = {}) => ({ graph: KICK, description: DESCRIBE, layoutOf, ...extra });

test('nothing selected: the project in numbers, the level shown and the legend', () => {
  const model = sidebar([], context());
  assert.equal(model.kind, 'project');
  assert.equal(field(model, 'instances'), '3');
  assert.equal(field(model, 'connections'), '2');
  assert.equal(field(model, 'composites'), '2');
  assert.equal(field(model, 'level shown'), 'top level');
  assert.equal(model.legend, LEGEND);
  assert.deepEqual(LEGEND.map((e) => e.label).slice(0, 4), ['signal', 'events', 'vector', 'converter (rate changes)']);
  const inside = sidebar([], context({ shown: { path: '/drums/kick', level: 'kick' } }));
  assert.equal(field(inside, 'level shown'), '/drums/kick (kick)');
  assert.equal(field(inside, 'its instances'), '3');
  assert.equal(field(inside, 'its connections'), '1');
  assert.equal(field(inside, 'instances of it'), '1');
  assert.equal(sidebar([{ kind: 'background' }], context()).kind, 'project', 'the background is nothing');
  assert.equal(sidebar([], {}).title, 'No schematic');
});

test('an instance: what it is, its pins with their connections as links, its parameters, the menu’s actions', () => {
  const model = sidebar([box('out')], context({ editing: true }));
  assert.equal(model.kind, 'instance');
  assert.equal(field(model, 'address'), '/out');
  assert.equal(field(model, 'module'), 'std.audio_out');
  assert.equal(field(model, 'composite'), 'no');
  assert.equal(field(model, 'level'), 'top level');
  assert.deepEqual(model.pins.map((p) => [p.name, p.role, p.connected, p.connections.length]), [['l', 'input', true, 1], ['r', 'input', false, 0], ['gain', 'parameter', false, 0]]);
  const [l] = model.pins;
  assert.equal(hitKey(l.hit), 'port /out.l in');
  assert.equal(l.connections[0].wire.label, '/drums.out →');
  assert.equal(hitKey(l.connections[0].wire.hit), 'wire  /drums.out /out.l', 'the link selects that wire');
  assert.equal(hitKey(l.connections[0].end.hit), 'port /drums.out out', 'and the other end');
  assert.equal(l.connections[0].removable, true);
  assert.deepEqual(model.parameters.map((p) => [p.address, p.editable]), [['/out.gain', true]], 'the card’s rows, editable at the top level');
  assert.deepEqual(ids(model), ['trace', 'copy-address', 'delete'], 'a primitive has no inside: no Enter, no Open');
  assert.equal(action(model, 'delete').disabled, false);
  const off = sidebar([box('out')], context());
  assert.equal(action(off, 'delete').title, EDIT_OFF, 'the same reason as the menu’s');

  const drums = sidebar([box('drums')], context({ menu: () => ({ open: true, wide: false }) }));
  assert.equal(field(drums, 'composite'), 'yes, 1 instance inside');
  assert.deepEqual(ids(drums), ['trace', 'enter', 'close', 'copy-address', 'delete'], 'an open composite closes');
  const kick = instanceHit({ path: '/drums', level: 'drums' }, nodeOf('drums', 'kick'));
  const inner = sidebar([kick], context());
  assert.equal(field(inner, 'level'), 'composite drums, 1 instance');
  assert.ok(inner.parameters.every((p) => !p.editable), 'read-only inside a composite');
});

test('a primitive shows what its type is (its formula) and its category; a composite its group', () => {
  const types = moduleInfo({
    primitives: [{ module: 'std.audio_out', composite: false, category: 'io', description: 'Stereo audio sink: the driver records gain·l and gain·r.' }],
    composites: [{ module: 'drums', composite: true, description: null }],
  });
  const out = sidebar([box('out')], context({ types }));
  assert.equal(out.about, 'Stereo audio sink: the driver records gain·l and gain·r.');
  assert.equal(field(out, 'category'), 'io');
  assert.deepEqual(out.fields.map(([l]) => l), ['address', 'module', 'category', 'composite', 'level']);
  const unknown = sidebar([box('out')], context());
  assert.equal(unknown.about, null, 'until the types are known: nothing');
  assert.equal(field(unknown, 'category'), '—');
  const drums = sidebar([box('drums')], context());
  assert.equal(drums.about, null, 'a composite has no formula');
  assert.equal(field(drums, 'category'), 'project', 'known without the types');
  assert.equal(sidebar([wireOf('/lfo.out')], context({ types })).about, undefined, 'a wire: none');
});

test('a wire: its ends as links, shape, rate, scale, converters; Edit scale and Delete', () => {
  const model = sidebar([wireOf('/lfo.out')], context({ editing: true }));
  assert.equal(model.kind, 'wire');
  assert.equal(field(model, 'shape'), 'signal');
  assert.equal(field(model, 'rate'), 'divisor 1');
  assert.equal(field(model, 'scale'), '120');
  assert.equal(field(model, 'converter'), 'none');
  assert.deepEqual(model.links.map((l) => [l.name, l.label, hitKey(l.hit)]), [
    ['from', '/lfo.out', 'port /lfo.out out'],
    ['to', '/drums.kick_decay', 'port /drums.kick_decay in'],
  ]);
  assert.deepEqual(ids(model), ['trace', 'scale', 'copy-address', 'delete']);
  assert.ok(model.actions.every((a) => !a.disabled));
  // An export: its end on the frame selects the frame's pin (ADR 0030 §1), and it is not edited.
  const exported = wireHit({ path: '/drums', level: 'drums' }, layoutOf('drums').wires.find((w) => w.kind === 'export' && w.from.node === '@inputs'));
  const frame = sidebar([exported], context({ editing: true }));
  assert.equal(hitKey(frame.links[0].hit), 'port /drums.trigger out');
  assert.equal(frame.links[0].hit.node.id, '@inputs');
  assert.equal(frame.links[0].label, '/drums.trigger');
  assert.equal(action(frame, 'delete').disabled, true);
});

test('a pin: what it is, its connections (removable in Edit patch), a level for a signal output, Disconnect all', () => {
  const lfo = nodeOf('', 'lfo');
  const out = sidebar([portHit(TOP, lfo, lfo.outs[0])], context({ editing: true, menu: () => ({ connected: 1 }) }));
  assert.equal(out.kind, 'port');
  assert.equal(field(out, 'direction'), 'output');
  assert.equal(field(out, 'connected'), '1 wire');
  assert.equal(out.meter, true);
  assert.deepEqual(out.connections.map((c) => [c.wire.label, hitKey(c.end.hit), c.removable]), [['→ /drums.kick_decay', 'port /drums.kick_decay in', true]]);
  assert.deepEqual(ids(out), ['trace', 'copy-address', 'disconnect']);
  assert.equal(action(out, 'disconnect').disabled, false);
  const outNode = nodeOf('', 'out');
  const gain = sidebar([portHit(TOP, outNode, outNode.ins.find((p) => p.name === 'gain'))], context());
  assert.equal(field(gain, 'direction'), 'parameter (in)');
  assert.equal(field(gain, 'unit'), '—');
  assert.equal(field(gain, 'connected'), 'no');
  assert.equal(gain.meter, false);
  assert.equal(action(gain, 'disconnect').disabled, true);
});

test('connections of a pin at a composite’s level: an export’s end on the frame selects the frame’s pin', () => {
  const entry = { path: '/drums/kick', level: 'kick', layout: layoutOf('kick') };
  const env = nodeOf('kick', 'env');
  const [trigger] = connectionsOf(entry, env, env.ins.find((p) => p.name === 'trigger'));
  assert.equal(hitKey(trigger.end.hit), 'port /drums/kick.trigger out');
  // The frame's pin itself: a boundary pin, its connections inside.
  const pin = sidebar([trigger.end.hit], context({ layoutOf }));
  assert.equal(field(pin, 'direction'), 'exported input (boundary)');
  assert.match(pin.subtitle, /^boundary pin of \/drums\/kick/);
  assert.equal(pin.connections.length, 1);
  assert.equal(pin.meter, false);
  assert.equal(trigger.removable, false);
  const osc = nodeOf('kick', 'osc');
  const [toDc] = connectionsOf(entry, osc, osc.outs[0]);
  assert.equal(hitKey(toDc.end.hit), 'port /drums/kick/dc.in in');
});

test('several parts: counts, the members as links, Delete all per level, Copy addresses', () => {
  const osc = instanceHit({ path: '/drums/kick', level: 'kick' }, nodeOf('kick', 'osc'));
  const lfo = nodeOf('', 'lfo');
  const selected = [box('lfo'), wireOf('/drums.out'), osc, portHit(TOP, lfo, lfo.outs[0])];
  const model = sidebar(selected, context({ editing: true }));
  assert.equal(model.kind, 'group');
  assert.equal(model.title, '4 selected');
  assert.equal(model.subtitle, '2 instances, 1 wire, 1 pin');
  assert.deepEqual(model.members.map((m) => m.label), ['/lfo', '/drums.out → /out.l', '/drums/kick/osc', '/lfo.out']);
  assert.equal(model.members[2].hit, osc);
  assert.deepEqual(model.plan.patches.map((p) => p.level), ['', 'kick']);
  assert.match(model.notes[0], /2 patches, one per level \(top level, kick\)/);
  assert.match(model.notes[1], /1 pin or export not deleted/);
  assert.deepEqual(ids(model), ['delete-all', 'copy-addresses']);
  assert.equal(action(model, 'delete-all').disabled, false);
  assert.equal(action(sidebar(selected, context()), 'delete-all').title, EDIT_OFF);
  const pins = sidebar([portHit(TOP, lfo, lfo.outs[0]), portHit(TOP, nodeOf('', 'out'), nodeOf('', 'out').ins[0])], context({ editing: true }));
  assert.equal(action(pins, 'delete-all').disabled, true, 'pins alone: nothing to delete');
  assert.equal(addressesText(selected), '/lfo\n/drums.out → /out.l\n/drums/kick/osc\n/lfo.out');
});

test('a large group: the first 200 members listed and the rest counted; a Delete over a message refused with its reason', () => {
  const many = Array.from({ length: 3000 }, (_, i) => ({ kind: 'instance', path: '', level: '', address: `/box_${i}`, node: { kind: 'instance', name: `box_${i}`, composite: false } }));
  const model = sidebar(many, context({ editing: true }));
  assert.equal(model.members.length, MEMBERS_SHOWN);
  assert.equal(model.more, 3000 - MEMBERS_SHOWN);
  assert.equal(action(model, 'delete-all').label, 'Delete 3000');
  assert.equal(action(model, 'delete-all').disabled, true);
  assert.match(action(model, 'delete-all').title, /over the 64 KiB a message \(and one undo\) may hold/);
  assert.equal(model.notes[0], tooLargeText(model.plan));
  const few = sidebar(many.slice(0, 5), context({ editing: true }));
  assert.equal(few.more, 0);
  assert.equal(action(few, 'delete-all').disabled, false);
  assert.equal(tooLargeText(few.plan), null);
});

test('the Trace section: the direction toggle, where it comes from and goes, the tree by level, each node revealing what it names', () => {
  assert.equal(traceSection(null, GRAPH), null);
  const model = traceSection(trace(GRAPH, { kind: 'pin', address: '/voice.hz' }, 'both'), GRAPH);
  assert.equal(model.title, '/voice.hz');
  assert.equal(model.subtitle, '3 levels, 5 wires, 6 pins');
  assert.deepEqual(model.directions.map((d) => [d.id, d.on]), [['up', false], ['down', false], ['both', true]]);
  assert.deepEqual(model.directions.map((d) => d.id), TRACE_DIRECTIONS.map((d) => d.id));
  assert.deepEqual(
    model.sides.map((s) => [s.name, s.title, s.count, s.endpoints.map((e) => e.label)]),
    [
      ['up', 'Comes from', 1, ['/seq.hz']],
      ['down', 'Goes to', 3, ['/voice/pitch/r1.in', '/voice/pitch/r2.in', '/voice/pitch/r3.in']],
    ],
  );
  assert.equal(model.sides[0].endpoints[0].note, 'std.note_seq');
  assert.deepEqual(model.sides[0].endpoints[0].reveal, { kind: 'pin', address: '/seq.hz' });
  assert.deepEqual(
    model.levels.map((l) => [l.label, l.count, l.reveal]),
    [
      ['top level', '1 wire, 2 pins', { kind: 'level', path: '' }],
      ['/voice (voice)', '1 wire, 1 pin', { kind: 'level', path: '/voice' }],
      ['/voice/pitch (pitch)', '3 wires, 3 pins', { kind: 'level', path: '/voice/pitch' }],
    ],
  );
  const voice = model.levels[0].children[1];
  assert.deepEqual([voice.label, voice.note, voice.reveal], ['voice', 'voice', { kind: 'instance', address: '/voice' }]);
  assert.deepEqual(voice.children, [{ label: 'hz', role: 'start', direction: 'in', reveal: { kind: 'pin', address: '/voice.hz' } }]);
  assert.deepEqual(model.notes, []);
});

test('the Trace section: one direction, an empty side, converters, a library composite noted', () => {
  const up = traceSection(trace(GRAPH, { kind: 'pin', address: '/voice/filt.cutoff' }, 'up'), GRAPH);
  assert.deepEqual(up.sides.map((s) => s.name), ['up']);
  assert.equal(up.sides[0].endpoints[0].note, 'std.osc; via converter out_d800');
  const none = traceSection(trace(GRAPH, { kind: 'pin', address: '/seq.hz' }, 'up'), GRAPH);
  assert.equal(none.sides[0].count, 0);
  assert.equal(none.sides[0].empty, 'nothing upstream');
  const lib = traceSection(trace(GRAPH, { kind: 'pin', address: '/mix.out' }, 'down'), GRAPH);
  assert.equal(lib.sides[0].endpoints[0].note, 'lib.tape_echo: library composite, inside not in the graph');
  assert.match(lib.notes[0], /^1 end at a library composite/);
  const fx = lib.levels[0].children.find((c) => c.label === 'fx');
  assert.equal(fx.note, 'lib.tape_echo (opaque)');
});

test('the Trace section: the depth toggle, the structural note, collapsed levels (M2.13-2)', () => {
  const next = traceSection(trace(GRAPH, { kind: 'pin', address: '/seq.hz' }, 'down'), GRAPH);
  assert.equal(next.depth, 'next');
  assert.deepEqual(
    next.depths.map((d) => [d.id, d.on]),
    [
      ['next', true],
      ['through', false],
    ],
  );
  assert.ok(!next.notes.some((n) => n.startsWith('Through modules')));
  const deep = traceSection(trace(GRAPH, { kind: 'pin', address: '/mix.a' }, 'up', { depth: 'through' }), GRAPH);
  assert.equal(deep.depths.find((d) => d.on).id, 'through');
  assert.equal(TRACE_DEPTHS.length, 2);
  assert.ok(deep.notes.some((n) => /structural path/.test(n)));
  assert.ok(deep.sides[0].endpoints.every((e) => /: source$/.test(e.note)), deep.sides[0].endpoints.map((e) => e.note).join(' | '));
  assert.ok(deep.levels.every((l) => l.collapsed === false));
});

test('a trace’s open ends are listed after its real ends, counted apart and marked', () => {
  const studio = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));
  const section = traceSection(trace(studio, { kind: 'pin', address: '/synth.hz' }, 'down', { depth: 'through' }), studio);
  const [down] = section.sides;
  assert.equal(down.title, 'Goes to');
  assert.equal(down.counted, '2, 2 open ends');
  assert.deepEqual(
    down.endpoints.map((e) => [e.label, e.open]),
    [
      ['/out.l', false],
      ['/out.r', false],
      ['/synth/ladder.lp12', true],
      ['/echo/tone.hp', true],
    ],
  );
  assert.match(down.endpoints[2].note, /^open end/);
  assert.match(down.endpoints[0].note, /sink$/);
});

/** The studio's `graph` (the owner's repro of 2026-10-20: /synth.gate selected at the top level). */
const STUDIO = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));

test('a composite box pin lists its inside too, each a link into the composite; Go inside (2026-10-20)', () => {
  const layouts = new Map();
  const studioLayout = (level) => {
    if (!layouts.has(level)) layouts.set(level, level === '' ? layout(STUDIO.top) : STUDIO.composites[level] ? layout(STUDIO.composites[level]) : null);
    return layouts.get(level);
  };
  const synth = studioLayout('').nodes.find((n) => n.name === 'synth');
  const gate = portHit(TOP, synth, synth.ins.find((p) => p.name === 'gate'));
  const model = sidebar([gate], { graph: STUDIO, layoutOf: studioLayout });
  // Outside, as before: the wire from the melody.
  assert.deepEqual(model.connections.map((c) => c.wire.label), ['/melody.gate →']);
  // Inside: every inner port the export forwards to, each revealing that pin.
  assert.equal(model.across.side, 'inside');
  assert.equal(model.across.title, 'Inside /synth');
  assert.deepEqual(model.across.items.map((i) => i.label), ['→ /synth/fenv.gate', '→ /synth/aenv.gate']);
  assert.deepEqual(model.across.items[1].reveal, { kind: 'pin', address: '/synth/aenv.gate' });
  assert.ok(ids(model).includes('go-inside') && !action(model, 'go-inside').disabled);
  // An exported parameter, and an output (where it comes from inside).
  const cutoff = sidebar([portHit(TOP, synth, synth.ins.find((p) => p.name === 'cutoff'))], { graph: STUDIO, layoutOf: studioLayout });
  assert.ok(cutoff.across.items.length >= 1 && cutoff.across.items.every((i) => i.label.startsWith('→ /synth/')), JSON.stringify(cutoff.across.items));
  const out = acrossOf(STUDIO, portHit(TOP, synth, synth.outs[0]));
  assert.deepEqual(out.items.map((i) => i.label), ['/synth/volume.out →']);
  // Inside the composite, its frame pin lists where its port connects outside, links that go back out; Go outside.
  const inner = { path: '/synth', level: 'moog_voice' };
  const frame = studioLayout('moog_voice').nodes.find((n) => n.kind === 'input');
  const fgate = portHit(inner, frame, frame.outs.find((p) => p.name === 'gate'));
  assert.equal(fgate.address, '/synth.gate');
  const fm = sidebar([fgate], { graph: STUDIO, layoutOf: studioLayout });
  assert.equal(fm.across.side, 'outside');
  assert.equal(fm.across.title, 'Outside, at the top level');
  assert.deepEqual(fm.across.items, [{ label: '/melody.gate →', reveal: { kind: 'pin', address: '/melody.gate' } }]);
  assert.ok(ids(fm).includes('go-outside') && !ids(fm).includes('go-inside'));
  // A primitive's pin has no other side.
  const melody = studioLayout('').nodes.find((n) => n.name === 'melody');
  assert.equal(acrossOf(STUDIO, portHit(TOP, melody, melody.outs[0])), null);
  assert.equal(sidebar([portHit(TOP, melody, melody.outs[0])], { graph: STUDIO, layoutOf: studioLayout }).across, null);
});

test('a library composite box pin: Go inside is off, its inside not in the graph', () => {
  const lay = layout(GRAPH.top);
  const fx = lay.nodes.find((n) => n.name === 'fx');
  const hit = portHit(TOP, fx, fx.ins[0]);
  const across = acrossOf(GRAPH, hit);
  assert.equal(across.opaque, true);
  assert.equal(across.items.length, 0);
  const model = sidebar([hit], { graph: GRAPH, layoutOf: (l) => (l === '' ? lay : null) });
  assert.ok(action(model, 'go-inside').disabled);
});

test('a library composite is said to be one, its inside not in the graph, and counted apart (the operator’s review of M2.13-3)', () => {
  const lib = new Map();
  const libOf = (level) => {
    if (!lib.has(level)) lib.set(level, level === '' ? layout(GRAPH.top) : GRAPH.composites[level] ? layout(GRAPH.composites[level]) : null);
    return lib.get(level);
  };
  const fx = instanceHit(TOP, libOf('').nodes.find((n) => n.name === 'fx'));
  const model = sidebar([fx], { graph: GRAPH, layoutOf: libOf });
  assert.equal(field(model, 'composite'), 'yes, a library composite (its inside is not in the graph)');
  assert.equal(action(sidebar([fx], { graph: GRAPH, layoutOf: libOf, menu: () => ({ opaque: true }) }), 'open').disabled, true, 'Open in place off');
  const project = sidebar([], { graph: GRAPH, layoutOf: libOf });
  assert.equal(field(project, 'library composites'), '1');
  assert.equal(field(sidebar([], { graph: KICK, layoutOf }), 'library composites'), undefined, 'none: no row');
});
