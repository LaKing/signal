// ADR 0046 §6 (test 7, the schematic), as the owner asked on 2026-10-30
// ("could a slot have a panel header with the same layout as on the panel
// itself?"): a slot's part is its face, inside its closed box under its
// title (the box grows to hold it at its measured size; its pins stay on
// its edges; no column of control nodes), at the top of its frame when it
// is opened, the inner structure below; a part's control on an inner
// address beside its inner instance; the host panel's controls still
// outside; box and face moved together by a drag.

import test from 'node:test';
import assert from 'node:assert/strict';
import { HEADER, layout, movedWires } from './schematic-layout.js';
import { openedLayout } from './schematic-open.js';
import { levelControls, levelFaces } from './parts.js';
import { FACE_W, estimateFace, labelColumn } from './schematic-face.js';
import { panelControls } from './schematic-controls.js';
import { hitTest } from './schematic-hit.js';
import { DragFollow } from './schematic-drag.js';
import { bake, bakedMatrix } from './schematic-draw.js';

const PART = {
  file: 'ui/parts/synth.html',
  controls: [
    { tag: 'signal-knob', label: 'cutoff', bind: '.cutoff', attributes: { bind: '.cutoff', label: 'cutoff' } },
    { tag: 'signal-scope', label: 'out', bind: '.out', attributes: { bind: '.out', label: 'out' } },
    { tag: 'signal-value', label: 'vcf', bind: '/vcf.cutoff', attributes: { bind: '/vcf.cutoff', label: 'vcf' } },
  ],
  error: null,
};

const synth = (part) => ({
  name: 'synth',
  address: '/synth',
  module: 'moog_voice',
  ports: [
    { name: 'gate', direction: 'in', shape: 'signal' },
    { name: 'out', direction: 'out', shape: 'signal' },
  ],
  parameters: [{ name: 'cutoff' }, { name: 'drive' }],
  slot: { interface: { ports: [] }, load: 'moog_voice', inside: { level: '/synth', own: false, make: true, name: 'synth_inside', reason: null }, part },
});

const graph = (part = PART) => ({
  top: {
    instances: [
      { name: 'lfo', address: '/lfo', module: 'std.sine_osc', ports: [{ name: 'out', direction: 'out', shape: 'signal' }], parameters: [] },
      synth(part),
      { name: 'echo', address: '/echo', module: 'std.gain', ports: [{ name: 'in', direction: 'in', shape: 'signal' }], parameters: [{ name: 'db' }] },
    ],
    connections: [
      { from: 'lfo.out', to: 'synth.cutoff' },
      { from: 'synth.out', to: 'echo.in' },
    ],
  },
  composites: {},
  insides: {
    '/synth': {
      slot: '/synth',
      inputs: [{ name: 'gate', shape: 'signal', to: [] }],
      parameters: [
        { name: 'cutoff', to: ['vcf.cutoff'] },
        { name: 'drive', to: [] },
      ],
      outputs: [{ name: 'out', shape: 'signal', from: 'vcf.out' }],
      instances: [
        {
          name: 'vcf',
          address: '/synth/vcf',
          module: 'std.ladder',
          ports: [
            { name: 'in', direction: 'in', shape: 'signal' },
            { name: 'out', direction: 'out', shape: 'signal' },
          ],
          parameters: [{ name: 'cutoff' }],
        },
      ],
      connections: [],
    },
  },
});

/** The host panel's knob on the slot's interface, still drawn outside the box. */
const HOST = panelControls('<signal-panel><div class="signal-cards"><section class="signal-card"><signal-knob bind="/synth.drive" label="host drive"></signal-knob></section></div></signal-panel>');


const node = (drawn, name) => drawn.nodes.find((n) => n.name === name);
const faced = (g, level, size = null) => ({ controls: levelControls(g, level, level === '' ? HOST : []), faces: levelFaces(g, level), faceSize: size ? () => size : null });

test('a closed slot shows its part as a face inside its box, under its title, its pins on its edges; no column of control nodes', () => {
  const g = graph();
  const bare = layout(graph(null).top);
  const drawn = layout(g.top, {}, faced(g, ''));
  const box = drawn.nodes.find((n) => n.kind === 'instance' && n.name === 'synth');
  assert.ok(box.face, 'the box has its face');
  assert.equal(box.face.file, 'parts/synth.html');
  assert.ok(box.w >= FACE_W && box.w > node(bare, 'synth').w, 'the box grows for its face');
  assert.equal(box.face.dy, HEADER, 'under the title row');
  assert.ok(box.face.dx >= labelColumn(box.ins), 'clear of the left pins’ names');
  assert.ok(box.face.dx + box.face.w <= box.w - labelColumn(box.outs) + 1, 'clear of the right pins’ names');
  assert.ok(box.h >= HEADER + box.face.h, 'as tall as its face');
  for (const p of box.ins) assert.equal(p.x, box.x, `${p.name} on the left edge`);
  for (const p of box.outs) assert.equal(p.x, box.x + box.w, `${p.name} on the right edge`);
  // The part's controls are the face's: none is a node, no binding wire runs inside the box.
  assert.equal(node(drawn, 'cutoff'), undefined);
  assert.equal(node(drawn, 'out'), undefined);
  assert.ok(!drawn.wires.some((w) => w.kind === 'binding' && (w.from.node === 'synth' || w.to.node === 'synth') && !w.label.startsWith('host drive')));
  // The inner address waits for the slot to be opened.
  assert.equal(node(drawn, 'vcf'), undefined);
  // The host panel's control on the slot stays outside the box.
  const host = node(drawn, 'host drive');
  assert.ok(host && host.x + host.w <= box.x, 'the host panel’s control left of the box');
  // Automation stays visible: the lfo's wire ends at the box's cutoff pin.
  const lfo = drawn.wires.find((w) => w.written.from === 'lfo.out');
  assert.equal(lfo.to.node, 'synth');
  assert.equal(lfo.to.port, 'cutoff');
  // A click in the face's area (where the HTML face is not shown, or forwarded from it) is the box's.
  const transform = { k: 1, x: 0, y: 0 };
  const hit = hitTest({ levels: [{ path: '', level: '', transform, layout: drawn, nodes: drawn.nodes, wires: drawn.wires }] }, { x: box.x + box.face.dx + 20, y: box.y + box.face.dy + 20 });
  assert.equal(hit.kind, 'instance');
  assert.equal(hit.node.name, 'synth');
});

test('the face is laid out at its measured size: boxes grow with it and never overlap', () => {
  const g = graph();
  // A second slot in the same layer: both have faces.
  g.top.instances.push({ ...synth(PART), name: 'synth2', address: '/synth2' });
  g.top.connections.push({ from: 'lfo.out', to: 'synth2.cutoff' });
  const tall = layout(g.top, {}, faced(g, '', { w: FACE_W, h: 640 }));
  const a = tall.nodes.find((n) => n.name === 'synth');
  const b = tall.nodes.find((n) => n.name === 'synth2');
  assert.equal(a.face.h, 640);
  assert.ok(a.h >= HEADER + 640);
  assert.ok(a.y + a.h <= b.y || b.y + b.h <= a.y || a.x + a.w <= b.x || b.x + b.w <= a.x, 'the two faced boxes do not overlap');
  const short = layout(g.top, {}, faced(g, '', { w: FACE_W, h: 96 }));
  assert.ok(short.nodes.find((n) => n.name === 'synth').h < a.h, 'a shorter face, a shorter box');
  // Not measured yet: an estimate from its controls.
  const guessed = layout(g.top, {}, faced(g, ''));
  assert.equal(guessed.nodes.find((n) => n.name === 'synth').face.h, estimateFace(PART.controls.length).h);
});

test('opened in place, the frame shows the face at its top, the inner structure below it; inner controls beside their instance', () => {
  const g = graph();
  const drawn = layout(g.top, {}, faced(g, ''));
  const inside = layout(g.insides['/synth'], {}, faced(g, '/synth', { w: FACE_W, h: 200 }));
  assert.ok(inside.face, 'the inside has the face');
  assert.equal(inside.face.file, 'parts/synth.html');
  assert.equal(inside.face.h, 200);
  const top = Math.min(...inside.nodes.map((n) => n.y));
  assert.ok(inside.face.y + inside.face.h <= top, 'above every inner node');
  assert.ok(inside.bounds.y <= inside.face.y && inside.bounds.y + inside.bounds.h >= inside.face.y + inside.face.h, 'in the inside’s bounds, so the frame holds it');
  // The interface controls are no nodes in the frame; the inner one stands beside its instance.
  assert.equal(node(inside, 'cutoff'), undefined);
  const vcf = inside.nodes.find((n) => n.kind === 'control' && n.name === 'vcf');
  assert.equal(vcf.host, 'vcf', 'beside its inner instance');
  const opened = openedLayout(drawn, 'synth', inside.bounds);
  assert.ok(node(opened, 'host drive'), 'the host panel’s stays, beside the frame');
  // Without a face the inside is as before.
  const plain = layout(g.insides['/synth'], {}, { controls: levelControls(g, '/synth') });
  assert.equal(plain.face, null);
  assert.ok(plain.bounds.h < inside.bounds.h);
});

test('a drag moves the box, and with it its face, as one', () => {
  const g = graph();
  const drawn = layout(g.top, {}, faced(g, ''));
  const box = drawn.nodes.find((n) => n.kind === 'instance' && n.name === 'synth');
  const transform = { k: 1, x: 0, y: 0 };
  const baked = bake(transform, { w: 1000, h: 600 });
  const element = () => {
    const attributes = {};
    return { getAttribute: (n) => attributes[n] ?? null, setAttribute: (n, v) => (attributes[n] = String(v)), removeAttribute: (n) => delete attributes[n], attributes };
  };
  const group = {
    baked,
    placed: bakedMatrix(baked, transform),
    wireCache: new Map(drawn.wires.map((w) => [w.id, { el: element() }])),
    nodeCache: new Map(drawn.nodes.map((n) => [n.id, { el: element() }])),
  };
  const follow = new DragFollow('', 'synth');
  follow.apply({ layout: drawn, levels: [{ path: '', level: '', transform }], groups: new Map([['', group]]), to: { x: box.x + 30, y: box.y + 10 } });
  // The face's area is drawn in the box's group (the HTML face is placed over that area), so it follows the box's translate.
  assert.equal(group.nodeCache.get('synth').el.attributes.transform, 'translate(30 10)');
  const routes = movedWires(drawn, 'synth', { x: box.x + 30, y: box.y + 10 });
  const lfo = routes.find((r) => r.wire.written.from === 'lfo.out');
  assert.equal(lfo.b.x, box.x + 30);
});
