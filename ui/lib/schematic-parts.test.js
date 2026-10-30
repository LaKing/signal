// ADR 0046 §6 (test 7, the schematic): a slot's part's controls in the
// schematic: on its interface inside its closed box (which grows; its pins
// stay on its edges; a short binding wire from each pin), in its frame when
// it is opened (beside the frame pins), on inner addresses beside their
// inner instance; the host panel's controls still outside; box and controls
// moved together by a drag.

import test from 'node:test';
import assert from 'node:assert/strict';
import { layout, movedWires } from './schematic-layout.js';
import { openedLayout } from './schematic-open.js';
import { levelControls } from './parts.js';
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
const inBox = (inner, box) => inner.x >= box.x && inner.x + inner.w <= box.x + box.w && inner.y >= box.y && inner.y + inner.h <= box.y + box.h;

test("a closed slot holds its part's interface controls inside one bigger box, its pins on its edges", () => {
  const g = graph();
  const bare = layout(graph(null).top);
  const drawn = layout(g.top, {}, { controls: levelControls(g, '', HOST) });
  const box = drawn.nodes.find((n) => n.kind === 'instance' && n.name === 'synth');
  assert.ok(box.w > node(bare, 'synth').w, 'the box grows for its controls');
  const cutoff = node(drawn, 'cutoff');
  const out = node(drawn, 'out');
  assert.ok(cutoff.inside && out.inside);
  assert.ok(inBox(cutoff, box) && inBox(out, box), 'inside the box');
  assert.ok(cutoff.y + cutoff.h <= out.y || out.y + out.h <= cutoff.y, 'one above the other');
  for (const p of box.ins) assert.equal(p.x, box.x, `${p.name} on the left edge`);
  for (const p of box.outs) assert.equal(p.x, box.x + box.w, `${p.name} on the right edge`);
  // The inner address waits for the slot to be opened.
  assert.equal(node(drawn, 'vcf'), undefined);
  // The host panel's control on the slot stays outside the box.
  const host = node(drawn, 'host drive');
  assert.ok(host && host.x + host.w <= box.x, 'the host panel’s control left of the box');
  // The lfo's wire still reaches the box's pin; a short binding wire goes on from the pin to the knob.
  const lfo = drawn.wires.find((w) => w.written.from === '/lfo.out' || w.written.from === 'lfo.out');
  assert.equal(lfo.to.node, 'synth');
  const bind = drawn.wires.find((w) => w.kind === 'binding' && w.to.node === cutoff.id);
  const pin = box.ins.find((p) => p.name === 'cutoff');
  assert.deepEqual(bind.curve.start, { x: box.x, y: pin.y });
  assert.ok(!bind.feedback);
  const reader = drawn.wires.find((w) => w.kind === 'binding' && w.from.node === out.id);
  assert.equal(reader.curve.segs.at(-1).to.x, box.x + box.w);
  // A click on a control inside the box hits the control, not the box.
  const transform = { k: 1, x: 0, y: 0 };
  const hit = hitTest({ levels: [{ path: '', level: '', transform, layout: drawn, nodes: drawn.nodes, wires: drawn.wires }] }, { x: cutoff.x + cutoff.w / 2, y: cutoff.y + cutoff.h / 2 });
  assert.equal(hit.kind, 'control');
});

test("opened in place, the part's controls stand in the frame: beside the frame pins, and beside their inner instance", () => {
  const g = graph();
  const drawn = layout(g.top, {}, { controls: levelControls(g, '', HOST) });
  const inside = layout(g.insides['/synth'], {}, { controls: levelControls(g, '/synth') });
  const opened = openedLayout(drawn, 'synth', inside.bounds);
  assert.ok(!opened.nodes.some((n) => n.inside), 'no control is left in the box');
  assert.ok(!opened.wires.some((w) => w.kind === 'binding' && opened.nodes.every((n) => n.id !== w.to.node && n.id !== w.from.node)));
  assert.ok(node(opened, 'host drive'), 'the host panel’s stays, beside the frame');
  const inputs = inside.nodes.find((n) => n.id === '@inputs');
  const outputs = inside.nodes.find((n) => n.id === '@outputs');
  const cutoff = node(inside, 'cutoff');
  const out = node(inside, 'out');
  const vcf = inside.nodes.find((n) => n.kind === 'control' && n.name === 'vcf');
  assert.ok(cutoff.frame && cutoff.x >= inputs.x + inputs.w, 'right of the frame inputs');
  assert.ok(out.frame && out.x + out.w <= outputs.x, 'left of the frame outputs');
  assert.equal(vcf.host, 'vcf', 'beside its inner instance');
  const pin = inputs.outs.find((p) => p.name === 'cutoff');
  const bind = inside.wires.find((w) => w.kind === 'binding' && w.to.node === cutoff.id);
  assert.deepEqual(bind.curve.start, { x: pin.x, y: pin.y });
});

test('a drag moves the box and the controls inside it together', () => {
  const g = graph();
  const drawn = layout(g.top, {}, { controls: levelControls(g, '', HOST) });
  const cutoff = node(drawn, 'cutoff');
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
  assert.equal(group.nodeCache.get(cutoff.id).el.attributes.transform, 'translate(30 10)');
  assert.equal(group.nodeCache.get('synth').el.attributes.transform, 'translate(30 10)');
  // The binding wire inside moves whole; the lfo's wire follows the pin.
  const routes = movedWires(drawn, new Map([['synth', { x: box.x + 30, y: box.y + 10 }], [cutoff.id, { x: cutoff.x + 30, y: cutoff.y + 10 }]]));
  const bind = routes.find((r) => r.wire.kind === 'binding' && r.wire.to.node === cutoff.id);
  assert.deepEqual(bind.a, { x: box.x + 30, y: bind.wire.curve.start.y + 10 });
});
