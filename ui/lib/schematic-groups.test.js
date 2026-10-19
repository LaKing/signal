// Ports in groups (M2.13-2, ADR 0030 §3, docs/ui-controls.md "Schematic"):
// a composite's pins ordered and grouped by the export's `group`, else its
// prefix, in the file's export order; collapsed at small scale to one pin
// per group, its wires joining that pin as a bundle; the labels of a
// collapsed box name its groups; the studio's voice.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { GROUP_PX, collapsedAt, exportFacts, groupAnchors, orderedPins, portGroups } from './schematic-groups.js';
import { ROW, layout } from './schematic-layout.js';
import { bundleWires } from './schematic-bundle.js';
import { boxLabels, overlaps } from './schematic-labels.js';
import { hitTest } from './schematic-hit.js';
import { startOf, trace } from './schematic-trace.js';
import { schematicMenu } from './schematic-menu.js';
import { SchematicEditor } from '../controls/schematic-editor.js';
import { traceTarget } from './schematic-select.js';
import { sidebar } from './schematic-sidebar.js';

/** The studio's `graph` structure (crates/signal-project/tests/structure.rs keeps it equal to `Project::structure`). */
const STUDIO = JSON.parse(readFileSync(new URL('./fixtures/studio-graph.json', import.meta.url), 'utf8'));
const pins = (names) => names.map((name) => ({ name }));
const names = (list) => list.map((p) => p.name);

test('groups by prefix in the file’s export order; an explicit group wins', () => {
  const facts = exportFacts({
    inputs: [{ name: 'hz', order: 0 }, { name: 'gate', order: 1 }],
    parameters: [
      { name: 'f_attack', order: 0 },
      { name: 'osc1_octave', order: 1 },
      { name: 'f_decay', order: 2 },
      { name: 'osc1_level', order: 3 },
      { name: 'drive', order: 4, group: 'f_' },
      { name: 'spare', order: 5, group: 'misc' },
    ],
  });
  const groups = portGroups(pins(['drive', 'f_decay', 'gate', 'hz', 'osc1_level', 'osc1_octave', 'f_attack', 'spare']), facts);
  assert.deepEqual(
    groups.map((g) => [g.label, names(g.pins)]),
    [
      ['hz', ['hz']],
      ['gate', ['gate']],
      ['f_*', ['f_attack', 'f_decay']],
      ['osc1_*', ['osc1_octave', 'osc1_level']],
      ['f_', ['drive']],
      ['misc', ['spare']],
    ],
  );
  assert.deepEqual(names(orderedPins(pins(['f_decay', 'hz', 'f_attack']), facts)), ['hz', 'f_attack', 'f_decay']);
});

test('the studio voice’s box: its inputs, then its parameters in groups, in the file’s order', () => {
  const drawn = layout(STUDIO.top, {}, { factsOf: (m) => exportFacts(STUDIO.composites[m] ?? null) });
  const synth = drawn.nodes.find((n) => n.name === 'synth');
  assert.deepEqual(names(synth.ins).slice(0, 8), ['hz', 'gate', 'velocity', 'glide', 'osc1_octave', 'osc1_detune', 'osc1_wave', 'osc1_level']);
  const labels = synth.groups.ins.map((g) => `${g.label} ${g.pins.length}`);
  assert.deepEqual(labels.slice(0, 8), ['hz 1', 'gate 1', 'velocity 1', 'glide 1', 'osc1_* 4', 'osc2_* 4', 'osc3_* 4', 'noise_level 1']);
  assert.ok(labels.includes('f_* 4') && labels.includes('a_* 4'), labels.join(', '));
  // The pins follow the order: row by row down the box.
  for (let i = 1; i < synth.ins.length; i++) assert.ok(synth.ins[i].y > synth.ins[i - 1].y);
  // Without facts (as before), the layout is unchanged.
  const plain = layout(STUDIO.top).nodes.find((n) => n.name === 'synth');
  assert.equal(plain.groups, undefined);
});

test('small, a group collapses to one pin at its middle; zoomed in it is open', () => {
  assert.ok(collapsedAt((GROUP_PX - 1) / ROW));
  assert.ok(!collapsedAt(GROUP_PX / ROW));
  const drawn = layout(STUDIO.top, {}, { factsOf: (m) => exportFacts(STUDIO.composites[m] ?? null) });
  const synth = drawn.nodes.find((n) => n.name === 'synth');
  const anchors = groupAnchors(synth);
  const osc1 = synth.ins.filter((p) => p.name.startsWith('osc1_'));
  const a = anchors.get('osc1_octave');
  assert.equal(a.group.label, 'osc1_*');
  assert.equal(a.y, (osc1[0].y + osc1.at(-1).y) / 2);
  assert.ok(osc1.every((p) => anchors.get(p.name) === a));
  assert.equal(anchors.get('hz'), undefined, 'a pin alone keeps its place');
});

test('wires to a collapsed group join its pin as one bundle; a lone wire is drawn to it', () => {
  const port = (name, direction) => ({ name, direction, shape: 'signal', kind: 'generic', rate: 1 });
  const level = {
    instances: [
      { name: 'src', module: 'std.x', composite: false, ports: [port('a', 'out'), port('b', 'out'), port('c', 'out')], parameters: [] },
      { name: 'v', module: 'voice', composite: true, ports: [port('hz', 'in')], parameters: [{ name: 'f_attack' }, { name: 'f_decay' }, { name: 'f_sustain' }, { name: 'a_attack' }, { name: 'a_decay' }] },
    ],
    connections: [
      { from: '/src.a', to: '/v.f_attack', converters: [] },
      { from: '/src.b', to: '/v.f_decay', converters: [] },
      { from: '/src.c', to: '/v.a_decay', converters: [] },
    ],
  };
  const facts = exportFacts({ inputs: [{ name: 'hz', order: 0 }], parameters: ['f_attack', 'f_decay', 'f_sustain', 'a_attack', 'a_decay'].map((name, order) => ({ name, order })) });
  const drawn = layout(level, {}, { factsOf: () => facts });
  const v = drawn.nodes.find((n) => n.name === 'v');
  const anchors = groupAnchors(v);
  const ends = (w) => {
    const b = anchors.get(w.to.port);
    return b ? { a: w.curve.start, b: { x: b.x, y: b.y }, key: b.group.key } : null;
  };
  const k = (GROUP_PX - 1) / ROW;
  const items = bundleWires(drawn.wires, k, { bottom: drawn.bottom, ends });
  const bundle = items.find((i) => i.kind === 'bundle');
  assert.equal(bundle.count, 2);
  const f = anchors.get('f_attack');
  assert.deepEqual(bundle.curve.segs.at(-1).to, { x: f.x, y: f.y });
  const lone = items.find((i) => i.kind !== 'bundle');
  const a = anchors.get('a_decay');
  assert.deepEqual(lone.curve.segs.at(-1).to, { x: a.x, y: a.y });
  assert.equal(lone.id, drawn.wires.find((w) => w.to.port === 'a_decay').id, 'the wire keeps its id');
});

test('a collapsed box’s labels name its groups, never its hidden pins', () => {
  const groups = portGroups(pins(['hz', 'f_attack', 'f_decay', 'f_sustain', 'a_attack', 'a_decay']), new Map());
  const at = (list) => list.map((p, i) => ({ name: p.name, x: 0, y: 40 + 12 * i }));
  const ins = at(groups.flatMap((g) => g.pins));
  const box = { kind: 'instance', name: 'v', rect: { x: 0, y: 0, w: 200, h: 120 }, ins, outs: [], groups: { ins: groups, outs: [] }, collapsed: true };
  const { labels } = boxLabels(box, 0);
  assert.deepEqual(
    labels.map((l) => l.text),
    ['hz', 'f_* (3)', 'a_* (2)'],
  );
  for (let i = 0; i < labels.length; i++) for (let j = i + 1; j < labels.length; j++) assert.ok(!overlaps(labels[i], labels[j]));
  // Open (not collapsed), every pin has its own label.
  assert.equal(boxLabels({ ...box, collapsed: false }, 0).labels.length, 6);
});

/** The studio's top level as drawn at scale `k`, its composites' groups collapsed (`collapsed`) or not. */
function studioAt(k, collapsed) {
  const drawn = layout(STUDIO.top, {}, { factsOf: (m) => exportFacts(STUDIO.composites[m] ?? null) });
  const entry = { path: '', level: '', layout: drawn, transform: { k, x: 0, y: 0 }, nodes: drawn.nodes, wires: drawn.wires, collapsed };
  const synth = drawn.nodes.find((n) => n.name === 'synth');
  return { drawn, entry, synth, anchors: groupAnchors(synth) };
}

test('collapsed, a group is hit as drawn: its pin is the group, its members’ rows are not pins (auditor-1’s probe at k = 0.5)', () => {
  // auditor-1's probe: the studio drawn collapsed at k = 0.5 (a row is 8 px there, below the 11 px of a label: collapsed).
  const k = 0.5;
  assert.ok(collapsedAt(k));
  const { entry, synth, anchors } = studioAt(k, true);
  const a = anchors.get('osc1_octave');
  const atAnchor = hitTest({ levels: [entry] }, { x: a.x * k, y: a.y * k });
  assert.equal(atAnchor.kind, 'group');
  assert.equal(atAnchor.label, 'osc1_*');
  assert.deepEqual(
    atAnchor.ports.map((p) => p.address),
    ['/synth.osc1_octave', '/synth.osc1_detune', '/synth.osc1_wave', '/synth.osc1_level'],
  );
  const level = synth.ins.find((p) => p.name === 'osc1_level');
  const atHidden = hitTest({ levels: [entry] }, { x: level.x * k, y: level.y * k });
  assert.ok(atHidden.kind !== 'port' || !atHidden.address.includes('osc1_'), JSON.stringify(atHidden.address));
  assert.notEqual(atHidden.address, '/synth.osc1_level');
  // A pin alone stays a pin.
  const hz = synth.ins.find((p) => p.name === 'hz');
  assert.equal(hitTest({ levels: [entry] }, { x: hz.x * k, y: hz.y * k }).address, '/synth.hz');
  // Open (zoomed in), the members are pins again.
  const open = studioAt(k, false);
  assert.equal(hitTest({ levels: [open.entry] }, { x: level.x * k, y: level.y * k }).address, '/synth.osc1_level');
});

test('a collapsed group is traced and inspected as its members, never one of them', () => {
  const k = 0.5;
  const { entry, anchors } = studioAt(k, true);
  const a = anchors.get('f_attack');
  const hit = hitTest({ levels: [entry] }, { x: a.x * k, y: a.y * k });
  assert.equal(hit.kind, 'group');
  const start = startOf(hit);
  assert.equal(start.kind, 'pins');
  const t = trace(STUDIO, start, 'down');
  for (const p of hit.ports) assert.ok(t.pins.has(p.address), p.address);
  assert.ok(t.pins.has('/synth/fenv.attack_ms') && t.pins.has('/synth/fenv.release_ms'));
  assert.deepEqual(
    schematicMenu(hit, {}).map((i) => i.id),
    ['inspect', 'trace'],
  );
});

test('a patch dropped on a collapsed group pin writes no connection', () => {
  const k = 0.5;
  const { entry, anchors, drawn } = studioAt(k, true);
  const a = anchors.get('osc1_octave');
  const drop = hitTest({ levels: [entry] }, { x: a.x * k, y: a.y * k });
  assert.equal(drop.kind, 'group');
  // The editor as <signal-schematic> drives it: a connection started at an output, ended over `drop`.
  const melody = drawn.nodes.find((n) => n.name === 'melody');
  const source = { kind: 'port', path: '', level: '', node: melody, port: melody.outs[0], address: `/melody.${melody.outs[0].name}` };
  const editor = Object.create(SchematicEditor.prototype);
  const written = [];
  const said = [];
  Object.assign(editor, { connecting: { hit: source, keys: new Set(drop.ports.map((p) => p.address)), pins: [] }, cord: null, s: { say: (m) => said.push(m) }, write: (...args) => written.push(args) });
  editor.connectEnd(drop);
  assert.deepEqual(written, []);
});

test('zooming out the studio synth: every pin named, then its groups named, then one summary; never pins without names', () => {
  const drawn = layout(STUDIO.top, {}, { factsOf: (m) => exportFacts(STUDIO.composites[m] ?? null) });
  const synth = drawn.nodes.find((n) => n.name === 'synth');
  const seen = [];
  for (let k = 1.2; k >= 0.2; k -= 0.02) {
    const collapsed = collapsedAt(k);
    const at = (p) => ({ name: p.name, x: p.x * k, y: p.y * k });
    const box = { kind: 'instance', name: 'synth', rect: { x: synth.x * k, y: synth.y * k, w: synth.w * k, h: synth.h * k }, ins: synth.ins.map(at), outs: synth.outs.map(at), groups: synth.groups, collapsed };
    const { level, labels } = boxLabels(box, 0);
    const state = level === 5 ? 'summary' : collapsed ? 'groups' : 'pins';
    if (seen.at(-1)?.state !== state) seen.push({ state, k: +k.toFixed(2) });
    // Pins drawn on their own are each named.
    if (!collapsed) assert.equal(labels.length, synth.ins.length + synth.outs.length, `k ${k.toFixed(2)}: ${labels.map((l) => l.text).join(' | ')}`);
    if (state === 'groups') assert.ok(labels.some((l) => /^osc1_\*/.test(l.shown)), `k ${k.toFixed(2)}: ${labels.map((l) => l.shown).join(' | ')}`);
  }
  assert.deepEqual(
    seen.map((s) => s.state),
    ['pins', 'groups', 'summary'],
    JSON.stringify(seen),
  );
  // The groups hold over a range of scale, not a step.
  const [, groups, summary] = seen;
  assert.ok(groups.k - summary.k >= 0.12, JSON.stringify(seen));
});

test('a composite’s frame orders its pins as its box outside does: by group, in export order', () => {
  const factsOf = (m) => exportFacts(STUDIO.composites[m] ?? null);
  const box = layout(STUDIO.top, {}, { factsOf }).nodes.find((n) => n.name === 'synth');
  const inside = layout(STUDIO.composites.moog_voice, {}, { factsOf });
  const frameIns = inside.nodes.find((n) => n.kind === 'input');
  const frameOuts = inside.nodes.find((n) => n.kind === 'output');
  assert.deepEqual(
    frameIns.outs.map((p) => p.name),
    box.ins.map((p) => p.name),
  );
  assert.deepEqual(
    frameOuts.ins.map((p) => p.name),
    box.outs.map((p) => p.name),
  );
  assert.deepEqual(
    frameIns.outs.slice(0, 5).map((p) => p.name),
    ['hz', 'gate', 'velocity', 'glide', 'osc1_octave'],
  );
});

test('a collapsed group clicked selects its members; T, the sidebar and the menu all take the whole group', () => {
  const k = 0.5;
  const { entry, anchors } = studioAt(k, true);
  const a = anchors.get('osc1_octave');
  const hit = hitTest({ levels: [entry] }, { x: a.x * k, y: a.y * k });
  // The control selects `hit.ports` on a click: each knows its group.
  const selected = hit.ports;
  assert.ok(selected.every((p) => p.group === 'osc1_*'));
  const target = traceTarget(selected);
  assert.equal(target.kind, 'group');
  assert.deepEqual(startOf(target), startOf(hit), 'T traces what the menu traces');
  assert.equal(startOf(target).label, '/synth osc1_*');
  const side = sidebar(selected, {});
  assert.equal(side.pinGroup, true);
  assert.equal(side.title, '/synth osc1_* (4)');
  assert.ok(side.actions.some((a) => a.id === 'trace'));
  // One pin selected is traced alone; pins of two boxes together, by their count.
  assert.equal(traceTarget([selected[0]]), selected[0]);
  const hz = hitTest({ levels: [entry] }, (() => { const p = entry.nodes.find((n) => n.name === 'melody').outs[0]; return { x: p.x * k, y: p.y * k }; })());
  const mixed = traceTarget([selected[0], hz]);
  assert.equal(startOf(mixed).label, '2 pins');
  assert.equal(sidebar([selected[0], hz], {}).pinGroup, false);
});
