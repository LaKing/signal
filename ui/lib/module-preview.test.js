// The Add module browser's previews (docs/ui-controls.md "Editing the
// patch"): the panel card a type's instance would get, from a `describe` of
// it alone; its box and a composite's inside as the schematic lays them out;
// and the connection of the preview's controls, which sends nothing.

import test from 'node:test';
import assert from 'node:assert/strict';
import { browserEntries } from './module-browser.js';
import { findParameter } from '../signal-ui.js';
import { PREVIEW_MARGIN, PreviewConnection, insideLevel, nodeLevel, previewDescribe, previewPanel, previewShapes, storedPositions } from './module-preview.js';
import { HEADER, ROW, PAD } from './schematic-layout.js';

const WAVES = [
  { value: 0, label: 'sine' },
  { value: 1, label: 'saw' },
];

const [OSC, VOICE] = browserEntries({
  primitives: [
    {
      module: 'std.osc',
      composite: false,
      category: 'generator',
      description: 'An oscillator.',
      ports: [
        { name: 'reset', direction: 'in', shape: 'events', rate: 1, kind: 'trigger', unit: null, range: null },
        { name: 'out', direction: 'out', shape: 'signal', rate: 1, kind: 'audio', unit: null, range: null },
      ],
      parameters: [
        { name: 'freq', kind: 'frequency', rate: 1, unit: 'Hz', range: { min: 0, max: 24000 }, default: 440, combine: 'add' },
        { name: 'wave', kind: 'control', rate: 1, unit: null, range: { min: 0, max: 1 }, default: 1, combine: 'replace', values: WAVES },
        { name: 'level', kind: 'gain', rate: 1, unit: null, range: { min: 0, max: 4 }, default: 1, combine: 'multiply' },
      ],
    },
  ],
  composites: [
    {
      module: 'voice',
      composite: true,
      description: 'A voice.',
      ports: [
        { name: 'gate', direction: 'in', shape: 'signal', rate: 1, kind: 'gate', unit: null, range: null },
        { name: 'out', direction: 'out', shape: 'signal', rate: 1, kind: 'audio', unit: null, range: null },
      ],
      parameters: [{ name: 'cutoff', kind: 'frequency', rate: 1, unit: 'Hz', range: { min: 20, max: 20000 }, default: 800, combine: 'add' }],
      inside: {
        instances: [
          { name: 'osc', address: 'osc', module: 'std.sine_osc', composite: false, ports: [{ name: 'out', address: 'osc.out', direction: 'out', shape: 'signal', rate: 1, kind: 'audio' }], parameters: [] },
          { name: 'vca', address: 'vca', module: 'std.mul', composite: false, ports: [{ name: 'a', address: 'vca.a', direction: 'in', shape: 'signal' }, { name: 'b', address: 'vca.b', direction: 'in', shape: 'signal' }, { name: 'out', address: 'vca.out', direction: 'out', shape: 'signal' }], parameters: [] },
        ],
        connections: [{ from: 'osc.out', to: 'vca.a', scale: 1, converters: [] }],
        inputs: [{ name: 'gate', to: ['vca.b'], direction: 'in', shape: 'signal' }],
        outputs: [{ name: 'out', from: 'vca.out', direction: 'out', shape: 'signal' }],
        parameters: [],
      },
    },
  ],
});

/** Every shape of a preview, groups opened. */
function flat(shapes) {
  return shapes.flatMap((s) => [s, ...flat(s.children ?? [])]);
}

test('the panel preview: the generated panel’s card of the instance, a control per parameter at its default', () => {
  const description = previewDescribe(OSC, 'osc');
  assert.deepEqual(description.controls.map((c) => [c.address, c.min, c.max, c.default, c.base, c.unit]), [
    ['/osc.freq', 0, 24000, 440, 440, 'Hz'],
    ['/osc.wave', 0, 1, 1, 1, null],
    ['/osc.level', 0, 4, 1, 1, null],
  ]);
  assert.deepEqual(description.controls[1].values, WAVES, 'a value set kept (ADR 0043)');
  assert.deepEqual(description.sinks, [], 'no sink: no output card');
  // What a control reads of it, as of any project's describe.
  assert.deepEqual(findParameter(description, '/osc.freq'), { min: 0, max: 24000, default: 440, unit: 'Hz', base: 440, control: true, kind: 'frequency' });
  const panel = previewPanel(OSC, 'osc');
  assert.equal(panel.groups.length, 1);
  assert.deepEqual([panel.groups[0].instance, panel.groups[0].module], ['osc', 'std.osc']);
  assert.deepEqual(panel.groups[0].controls.map((c) => [c.element, c.bind, c.taper]), [
    ['signal-knob', '/osc.freq', 'linear'],
    ['signal-knob', '/osc.wave', 'linear'],
    ['signal-fader', '/osc.level', 'db'],
  ], 'as the generated panel chooses: a fader with a dB taper for a gain');
  assert.deepEqual([panel.meters, panel.scope], [[], null]);
  assert.equal(previewPanel({ module: 'std.x', ports: [], parameters: [] }, 'x').groups.length, 0, 'no parameters: no card');
});

test('the preview’s connection sends nothing and refuses every edit', async () => {
  const connection = new PreviewConnection(previewDescribe(OSC, 'osc'));
  assert.equal(typeof connection.on('describe', () => {}), 'function');
  assert.equal(typeof connection.subscribe({ stream: 'parameters', addresses: ['/osc.freq'] }, () => {}), 'function');
  for (const call of [() => connection.set('/osc.freq', 1), () => connection.ramp('/osc.freq', 1), () => connection.commit('/osc.freq', 1), () => connection.request({ type: 'modules' })]) {
    await assert.rejects(call(), (error) => error.code === 'preview');
  }
  assert.equal(connection.sent, 4, 'counted, never sent');
});

test('the schematic preview of a primitive: its box as the schematic lays it out, its pins named', () => {
  const level = nodeLevel(OSC, 'osc');
  assert.equal(level.instances.length, 1);
  assert.equal(level.instances[0].ports[0].address, 'osc.reset');
  const { viewBox, shapes } = previewShapes(level);
  assert.equal(shapes.length, 1, 'one box, no wires');
  const box = shapes[0];
  assert.equal(box.tag, 'g');
  assert.equal(box.attrs.class, 'node');
  const rect = box.children.find((s) => s.attrs.class === 'box');
  // An input and three parameter pins on the left, one output: four rows.
  assert.equal(rect.attrs.height, HEADER + ROW * 4 + PAD);
  assert.deepEqual([viewBox.w, viewBox.h], [rect.attrs.width + 2 * PREVIEW_MARGIN, rect.attrs.height + 2 * PREVIEW_MARGIN]);
  const texts = box.children.filter((s) => s.tag === 'text').map((s) => s.text);
  assert.deepEqual(texts, ['osc', 'std.osc', 'reset', 'freq', 'wave', 'level', 'out']);
  const pins = box.children.filter((s) => s.tag === 'circle').map((s) => s.attrs.class);
  assert.deepEqual(pins, ['pin events', 'pin signal parameter', 'pin signal parameter', 'pin signal parameter', 'pin signal'], 'the schematic’s classes: shape, parameter, nothing connected');
});

test('the schematic preview of a composite: its box, and its inside with the frame’s pins and the wires', () => {
  const box = previewShapes(nodeLevel(VOICE, 'voice')).shapes[0];
  assert.equal(box.attrs.class, 'node composite');
  assert.ok(box.children.some((s) => s.attrs.class === 'frame'));
  assert.equal(insideLevel(OSC), null);
  const inside = previewShapes(insideLevel(VOICE));
  const all = flat(inside.shapes);
  const wires = all.filter((s) => s.tag === 'path');
  assert.equal(wires.length, 3, 'osc → vca, the input gate → vca.b, vca → the output out');
  assert.ok(wires.every((w) => w.attrs.class === 'wire signal' && /^M/.test(w.attrs.d)));
  const groups = inside.shapes.filter((s) => s.tag === 'g').map((g) => [g.attrs.class, g.children.find((c) => c.attrs.class === 'name').text]);
  assert.deepEqual(groups, [['node pins', 'inputs'], ['node', 'osc'], ['node', 'vca'], ['node pins', 'outputs']]);
  // Every pin a wire reaches is filled, as in the schematic.
  const vca = inside.shapes.find((g) => g.children?.some((c) => c.text === 'vca'));
  assert.deepEqual(vca.children.filter((c) => c.tag === 'circle').map((c) => c.attrs.class), ['pin signal connected', 'pin signal connected', 'pin signal connected']);
  // A project composite's stored positions are kept; a library one's are none.
  const graph = { layout: { positions: { voice: { osc: { x: 500, y: 0 } } } } };
  assert.deepEqual(storedPositions(graph, VOICE), { osc: { x: 500, y: 0 } });
  assert.deepEqual(storedPositions(graph, { ...VOICE, source: 'library' }), {});
  const moved = previewShapes(insideLevel(VOICE), storedPositions(graph, VOICE));
  const osc = moved.shapes.find((g) => g.children?.some((c) => c.text === 'osc')).children[0];
  assert.deepEqual([osc.attrs.x, osc.attrs.y], [500, 0]);
});
