// Selection and editing at the Engine layer (step 3 of the owner's Engine
// rebuild, 2026-11-03): hit items with full addresses at the instrument's
// level and inside a revealed chip, what a point hits (an item, a chip's or
// an instrument's empty space), the patch ops a connect, a removal and an
// add write (the top level for the instrument, the composite's level inside
// a chip, a chip's export left alone as the schematic leaves it), Escape out
// one chip level, and the instances kept in the instrument they were added
// to until they are joined to one.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { engineLayout, insideOf } from './plane-engine.js';
import {
  addOps,
  bindMoveOps,
  connectOps,
  dropTargets,
  engineItems,
  engineSelection,
  engineSubject,
  engineTarget,
  fullAddress,
  norm,
  outStep,
  placedOwners,
  removeOps,
  rewireOps,
  selectionKeys,
  wireKey,
} from './plane-engine-edit.js';

const STUDIO = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));

/** The mono synth's Engine (melody, synth) in a rectangle, and the synth chip's inside laid out in its card. */
function monoSynth() {
  const rect = { x: 0, y: 0, w: 1280, h: 420 };
  const laid = engineLayout({ entry: 'card:mono-synth', rect, names: ['melody', 'synth'], graph: STUDIO });
  const top = engineItems(laid, { entry: 'card:mono-synth' });
  const card = laid.cards.find((c) => c.name === 'synth');
  const inside = insideOf({ graph: STUDIO, card, pins: laid.pins, scale: laid.scale });
  const inner = engineItems(inside, { entry: 'card:mono-synth', level: card.module, prefix: '/synth', chip: card.id });
  return { rect, laid, top, card, inside, inner, all: [...top, ...inner] };
}

const pin = (items, address) => items.find((i) => i.kind === 'e-pin' && i.address === address);
const card = (items, address) => items.find((i) => i.kind === 'e-card' && i.address === address);

test('full addresses: the top level’s with its slash, inside a chip under the chip’s address, its own pins as the chip’s', () => {
  assert.equal(norm('synth.gate'), '/synth.gate');
  assert.equal(norm('/synth.gate'), '/synth.gate');
  assert.equal(fullAddress('', 'melody.hz'), '/melody.hz');
  assert.equal(fullAddress('/synth', 'fenv.gate'), '/synth/fenv.gate');
  assert.equal(fullAddress('/synth', '@inputs.gate'), '/synth.gate');
  assert.equal(fullAddress('/synth', '@outputs.out'), '/synth.out');
  assert.equal(fullAddress('/echo/hit', 'env.out'), '/echo/hit/env.out');
});

test('hit items at the instrument’s level: cards, pins, wires and edge dots, at the top level, the chip named', () => {
  const { top, laid } = monoSynth();
  const synth = card(top, '/synth');
  assert.equal(synth.chipId, 'card synth');
  assert.equal(synth.composite, true);
  assert.equal(synth.patchLevel, '');
  assert.equal(synth.module, 'moog_voice');
  assert.equal(card(top, '/melody').chipId, null);
  const gate = pin(top, '/synth.gate');
  assert.deepEqual([gate.side, gate.local, gate.patchLevel, gate.chip, gate.cardChip, gate.module], ['in', 'synth.gate', '', null, 'card synth', 'moog_voice']);
  const wire = top.find((i) => i.kind === 'e-wire' && i.from === '/melody.hz');
  assert.equal(wire.to, '/synth.hz');
  assert.equal(wire.key, wireKey('/melody.hz', '/synth.hz'));
  assert.deepEqual(wire.written, { from: 'melody.hz', to: 'synth.hz' });
  assert.equal(wire.frame, false);
  // The edge dots: a connection through the instrument's edge, keyed as its wire.
  const edges = top.filter((i) => i.kind === 'e-edge');
  assert.equal(edges.length, laid.edges.length);
  assert.ok(edges.some((e) => e.key === wireKey('/transport.tick', '/melody.tick')));
  assert.equal(new Set(top.map((i) => i.id)).size, top.length, 'ids unique');
});

test('hit items inside a chip: full addresses under it, its composite’s level, its own pins’ wires marked as exports, no edge dots', () => {
  const { inner } = monoSynth();
  const fenv = card(inner, '/synth/fenv');
  assert.deepEqual([fenv.patchLevel, fenv.prefix, fenv.chip, fenv.name], ['moog_voice', '/synth', 'card synth', 'fenv']);
  const g = pin(inner, '/synth/fenv.gate');
  assert.deepEqual([g.local, g.patchLevel, g.chip], ['fenv.gate', 'moog_voice', 'card synth']);
  const exported = inner.find((i) => i.kind === 'e-wire' && i.from === '/synth.gate' && i.to === '/synth/fenv.gate');
  assert.ok(exported, 'the chip’s input to its inner port');
  assert.equal(exported.frame, true);
  const plain = inner.find((i) => i.kind === 'e-wire' && i.from === '/synth/mixer.out');
  assert.deepEqual([plain.to, plain.frame, plain.written.from, plain.written.to], ['/synth/ladder.in', false, 'mixer.out', 'ladder.in']);
  assert.equal(inner.filter((i) => i.kind === 'e-edge').length, 0);
});

test('what a point hits: an inner card over the chip’s, the chip’s empty body, the instrument’s empty space, nothing outside', () => {
  const { all, top, rect, card: chipCard, inside } = monoSynth();
  const k = 4;
  const rects = [{ entry: 'card:mono-synth', ...rect }];
  const fenv = inside.cards.find((c) => c.name === 'fenv');
  const onFenv = { x: fenv.x + fenv.w / 2, y: fenv.y + fenv.h / 2 };
  assert.equal(engineTarget(all, onFenv, k, { revealed: new Set(['card synth']), rects }).address, '/synth/fenv');
  // The chip's body where nothing of its inside is: the composite's empty space.
  const b = chipCard.body;
  const empty = { x: b.x + b.w - 0.5, y: b.y + b.h - 0.5 };
  const space = engineTarget(all, empty, k, { revealed: new Set(['card synth']), rects });
  assert.deepEqual([space.kind, space.patchLevel, space.prefix, space.chip, space.entry], ['e-space', 'moog_voice', '/synth', 'card synth', 'card:mono-synth']);
  // Not revealed: the chip's card itself.
  assert.equal(engineTarget(top, empty, k, { revealed: new Set(), rects }).address, '/synth');
  // The instrument's empty space: its top level.
  const corner = { x: rect.x + 2, y: rect.y + rect.h - 2 };
  const at = engineTarget(top, corner, k, { rects });
  assert.deepEqual([at.kind, at.patchLevel, at.chip, at.entry], ['e-space', '', null, 'card:mono-synth']);
  assert.equal(engineTarget(top, { x: -50, y: -50 }, k, { rects }), null);
});

test('connect: an output and an input of one level, in either order; the top level for the instrument, the composite’s inside a chip', () => {
  const { all } = monoSynth();
  const out = pin(all, '/melody.hz');
  const into = pin(all, '/synth.velocity');
  const edit = connectOps(out, into);
  assert.deepEqual(edit, { level: '', ops: [{ op: 'connection.add', from: 'melody.hz', to: 'synth.velocity' }], what: '/melody.hz → /synth.velocity' });
  assert.deepEqual(connectOps(into, out), edit, 'dragged from the input');
  const inner = connectOps(pin(all, '/synth/noise.out'), pin(all, '/synth/ladder.cutoff_hz'));
  assert.deepEqual(inner, { level: 'moog_voice', ops: [{ op: 'connection.add', from: 'noise.out', to: 'ladder.cutoff_hz' }], what: '/synth/noise.out → /synth/ladder.cutoff_hz' });
});

test('a chip’s own pin to its inside: the composite’s export reaches it (export.connect); refused: two levels, two outputs, not a pin', () => {
  const { all } = monoSynth();
  const exported = connectOps(pin(all, '/synth.gate'), pin(all, '/synth/aenv.gate'));
  assert.deepEqual(exported, { level: 'moog_voice', ops: [{ op: 'export.connect', role: 'input', name: 'gate', port: 'aenv.gate' }], what: '/synth.gate → /synth/aenv.gate (moog_voice’s export)' });
  assert.deepEqual(connectOps(pin(all, '/synth/volume.out'), pin(all, '/synth.out')).ops, [{ op: 'export.connect', role: 'output', name: 'out', port: 'volume.out' }], 'an inner output to the chip’s output');
  assert.match(connectOps(pin(all, '/synth.gate'), pin(all, '/synth/volume.out')).error, /input or a parameter inside/);
  assert.match(connectOps(pin(all, '/melody.hz'), pin(all, '/synth/fenv.gate')).error, /different levels/);
  assert.match(connectOps(pin(all, '/melody.hz'), pin(all, '/melody.gate')).error, /both outputs/);
  assert.match(connectOps(pin(all, '/melody.hz'), card(all, '/synth')).error, /on a pin/);
});

test('remove: a card or a wire at its level, an edge dot’s connection at the top; a chip’s export is not removed here', () => {
  const { all } = monoSynth();
  assert.deepEqual(removeOps(card(all, '/synth/fenv')), { level: 'moog_voice', ops: [{ op: 'instance.remove', name: 'fenv' }], what: '/synth/fenv removed' });
  assert.deepEqual(removeOps(card(all, '/melody')), { level: '', ops: [{ op: 'instance.remove', name: 'melody' }], what: '/melody removed' });
  const inner = all.find((i) => i.kind === 'e-wire' && i.from === '/synth/mixer.out');
  assert.deepEqual(removeOps(inner).ops, [{ op: 'connection.remove', from: 'mixer.out', to: 'ladder.in' }]);
  assert.equal(removeOps(inner).level, 'moog_voice');
  const top = all.find((i) => i.kind === 'e-wire' && i.from === '/melody.hz');
  assert.deepEqual(removeOps(top), { level: '', ops: [{ op: 'connection.remove', from: 'melody.hz', to: 'synth.hz' }], what: '/melody.hz → /synth.hz removed' });
  const edge = all.find((i) => i.kind === 'e-edge' && i.to === '/melody.tick');
  assert.deepEqual(removeOps(edge).ops, [{ op: 'connection.remove', from: 'transport.tick', to: 'melody.tick' }]);
  const frame = all.find((i) => i.kind === 'e-wire' && i.frame && String(i.written.from).startsWith('@inputs.'));
  assert.equal(removeOps(frame).ops[0].op, 'export.disconnect', 'an export’s wire: its inner port let go');
  assert.match(removeOps(pin(all, '/melody.hz')).error, /Select a wire or a card/);
  // One dot for an output leaving to several places: its wire is removed, not the dot.
  const shared = engineItems({ edges: [{ id: 'edge out a.out', side: 'out', address: 'a.out', to: 'b.in', x: 0, y: 0 }, { id: 'edge out a.out', side: 'out', address: 'a.out', to: 'c.in', x: 0, y: 0 }] }, { entry: 'e' });
  assert.match(removeOps(shared[0]).error, /select the wire/);
});

test('add: at the instrument’s empty space the top level, in a chip’s its composite; no position (the Engine lays it out)', () => {
  assert.deepEqual(addOps({ kind: 'e-space', patchLevel: '', prefix: '' }, 'std.gain', 'trim'), { level: '', ops: [{ op: 'instance.add', name: 'trim', module: 'std.gain' }], what: '/trim (std.gain) added' });
  assert.deepEqual(addOps({ kind: 'e-space', patchLevel: 'moog_voice', prefix: '/synth' }, 'std.noise', 'noise2'), { level: 'moog_voice', ops: [{ op: 'instance.add', name: 'noise2', module: 'std.noise' }], what: '/synth/noise2 (std.noise) added' });
});

test('a wire dropped on a card: what fits there, in the level’s own names', () => {
  const { all } = monoSynth();
  const level = STUDIO.composites.moog_voice;
  const ladder = card(all, '/synth/ladder');
  const fromNoise = dropTargets(level, pin(all, '/synth/noise.out'), ladder);
  assert.ok(fromNoise.some((t) => t.from === 'noise.out' && t.to === 'ladder.in'));
  assert.ok(fromNoise.some((t) => t.to === 'ladder.cutoff_hz' && /parameter/.test(t.text)));
  assert.ok(fromNoise.every((t) => t.to.startsWith('ladder.')));
  const intoLadder = dropTargets(level, pin(all, '/synth/ladder.in'), card(all, '/synth/noise'));
  assert.deepEqual(intoLadder, [{ text: 'out', from: 'noise.out', to: 'ladder.in' }]);
  assert.deepEqual(dropTargets(level, pin(all, '/synth/noise.out'), card(all, '/melody')), [], 'another level');
});

test('the selection’s keys: its own; a Wiring cable the Engine’s wire of it, a port both pins; the Show in subject', () => {
  const { all } = monoSynth();
  const wire = all.find((i) => i.kind === 'e-wire' && i.from === '/melody.hz');
  const sel = engineSelection(wire);
  assert.deepEqual([...selectionKeys(sel)], [wire.key]);
  assert.equal(engineSelection({ kind: 'e-space' }), null);
  assert.deepEqual([...selectionKeys({ kind: 'cable', from: 'melody.hz', to: '/synth.hz' })], [wireKey('/melody.hz', '/synth.hz')]);
  assert.deepEqual([...selectionKeys({ kind: 'port', address: '/synth.out' })], ['pin in /synth.out', 'pin out /synth.out']);
  assert.equal(selectionKeys(null).size, 0);
  assert.deepEqual(engineSubject(card(all, '/synth/fenv')), { kind: 'instance', address: '/synth/fenv' });
  assert.deepEqual(engineSubject(pin(all, '/synth/fenv.gate')), { kind: 'port', address: '/synth/fenv.gate' });
  assert.deepEqual(engineSubject(wire), { kind: 'wire', from: '/melody.hz', to: '/synth.hz' });
  assert.equal(engineSubject(all.find((i) => i.kind === 'e-wire' && i.frame)), null);
});

test('Escape inside chips: out one chip level, from the outermost to its instrument; nothing outside a chip', () => {
  const a = { id: 'card echo' };
  const b = { id: 'card echo/card hit' };
  assert.equal(outStep([]), null);
  assert.equal(outStep(undefined), null);
  assert.deepEqual(outStep([a]), { instrument: a });
  assert.deepEqual(outStep([a, b]), { chip: a, part: 'body' });
});

test('an instance added from an instrument’s Engine stays in it until a connection or a control joins it to one', () => {
  const instances = [{ name: 'trim' }, { name: 'bus' }, { name: 'out', module: 'std.audio_out' }, { name: 'lost' }];
  const owner = new Map([['trim', null], ['bus', 'card:master'], ['out', null], ['lost', null]]);
  const at = (entry, seen = false) => ({ entry, seen });
  const placed = new Map([['trim', at('card:master')], ['bus', at('card:echo')], ['out', at('card:master')], ['lost', at('card:gone')], ['coming', at('card:master')], ['undone', at('card:master', true)]]);
  const { owner: next, forget, seen } = placedOwners(owner, placed, instances, new Set(['card:master', 'card:echo']));
  assert.deepEqual(seen, ['trim']);
  assert.equal(next.get('trim'), 'card:master', 'in none: kept where it was added');
  assert.equal(next.get('bus'), 'card:master', 'joined one: the assignment’s');
  assert.equal(next.get('out'), null, 'a device slot is no instrument’s');
  assert.equal(next.get('lost'), null);
  assert.deepEqual(forget.sort(), ['bus', 'lost', 'out', 'undone'], 'gone after it was seen: forgotten');
  assert.equal(owner.get('trim'), null, 'the assignment itself unchanged');
  assert.ok(!next.has('coming'), 'not in the graph yet: kept, nothing assigned');
});

test('a wire’s end moved onto another pin: the old connection removed and the new one added in one patch', () => {
  const wire = { kind: 'e-wire', from: '/melody.gate', to: '/synth.gate', written: { from: 'melody.gate', to: 'synth.gate' }, patchLevel: '', chip: null, frame: false };
  const pin = (side, card, port, extra = {}) => ({ kind: 'e-pin', side, card, port, address: `/${card}.${port}`, local: `${card}.${port}`, chip: null, cardChip: null, patchLevel: '', ...extra });
  const moved = rewireOps(wire, 'to', pin('in', 'synth', 'velocity'));
  assert.equal(moved.level, '');
  assert.deepEqual(moved.ops.map((o) => [o.op, o.from, o.to]), [['connection.remove', 'melody.gate', 'synth.gate'], ['connection.add', 'melody.gate', 'synth.velocity']]);
  const source = rewireOps(wire, 'from', pin('out', 'melody', 'hz'));
  assert.deepEqual(source.ops[1].from, 'melody.hz');
  assert.ok(rewireOps(wire, 'to', pin('out', 'melody', 'hz')).error, 'an end on an input only');
  assert.ok(rewireOps(wire, 'to', pin('in', 'synth', 'gate')).error, 'already there');
  assert.ok(rewireOps(wire, 'to', pin('in', 'ladder', 'in', { chip: 'synth' })).error, 'another level');
  assert.ok(rewireOps({ ...wire, frame: true }, 'to', pin('in', 'synth', 'velocity')).error, 'an export is edited in its slot');
});

test('an export’s port moved inside a chip: the new port taken first, then the old let go, so the export keeps its range', () => {
  const bind = { kind: 'e-bind', prefix: '/synth', patchLevel: 'moog_voice', export: 'cutoff', local: 'ladder.cutoff_hz' };
  assert.deepEqual(bindMoveOps(bind, 'ladder.drive').ops, [
    { op: 'export.connect', role: 'parameter', name: 'cutoff', port: 'ladder.drive' },
    { op: 'export.disconnect', role: 'parameter', name: 'cutoff', port: 'ladder.cutoff_hz' },
  ]);
  assert.ok(bindMoveOps(bind, 'ladder.cutoff_hz').error, 'already there');
  assert.ok(bindMoveOps({ ...bind, prefix: '' }, 'ladder.drive').error, 'a leg at the package is a control’s bind, not an export');
});
